use crate::javascript_dom;
use hull_quickjs::Engine;
use std::{cell::RefCell, rc::Rc};
fn fixture() -> Engine {
    let mut engine = Engine::new().unwrap();
    javascript_dom::install(
        &mut engine,
        Rc::new(RefCell::new(rp5::parse(
            "<div id=a style='width:2px;color:black'></div><div id=b></div>",
        ))),
    )
    .unwrap();
    engine
}
fn run(engine: &mut Engine, script: &str) {
    engine.eval(script.as_bytes(), "style-test", false).unwrap();
}

#[test]
fn style_locks_span_sources_and_reset_on_navigation() {
    use crate::{
        javascript::{Report, Runtime},
        scripts::Scripts,
    };
    let mut runtime = Runtime::start().unwrap();
    for (generation, width) in [(1, 10), (2, 30)] {
        let html = format!(
            "<p>styled</p><script>document.querySelector('p').style.width='{width}px'</script><script>document.querySelector('p').style.width='99px';if(document.querySelector('p').style.width!=='{width}px')throw Error('field reset or changed')</script>"
        );
        let doc = rp5::parse(&html);
        let mut scripts = Scripts::default();
        scripts.extract(&doc, "https://fixture.test/style");
        runtime.begin_page(
            generation,
            "https://fixture.test/style",
            scripts.sources,
            doc,
        );
        let mut frozen = None;
        for _ in 0..256 {
            for report in runtime.poll() {
                match report {
                    Report::Evaluated { result, .. } => assert!(result.is_ok(), "{result:?}"),
                    Report::JobError { error, .. } => panic!("{error}"),
                    _ => {}
                }
            }
            if let Some(doc) = runtime.take_initial_document(generation) {
                frozen = Some(doc);
                break;
            }
        }
        let doc = frozen.expect("snapshot");
        let node = doc
            .nodes
            .iter()
            .enumerate()
            .find(|(id, _)| doc.name(*id) == "p")
            .unwrap()
            .0;
        assert!(
            doc.element(node)
                .unwrap()
                .2
                .iter()
                .any(|a| a.name == "style" && a.value.contains(&format!("{width}px")))
        );
    }
}

#[test]
fn inline_style_commits_first_valid_field_write_and_keeps_identity() {
    let mut engine = fixture();
    run(
        &mut engine,
        r#"
    const a=document.getElementById('a'),s=a.style;
    if(!(s instanceof CSSStyleDeclaration)||s!==a.style||s.width!=='2px'||s.parentRule!==null||!('width' in s)||('noSuchStyleField' in s))throw Error('declaration reads');
    s.width='invalid';s.width='20px';s.width='30px';if(s.width!=='20px')throw Error('first valid write');
    s.backgroundColor='red';s['background-color']='blue';if(s.backgroundColor!=='red')throw Error('alias');
    s.setProperty('overflow','hidden','important');s.setProperty('overflow','scroll');if(s.overflow!=='hidden'||s.getPropertyPriority('overflow')!=='important')throw Error('priority');
    document.getElementById('b').style.width='40px';if(document.getElementById('b').style.width!=='40px')throw Error('element scope');
    if(!a.getAttribute('style').includes('20px')||s.item(999)!==''||s[0]!==s.item(0)||[...s].length!==s.length)throw Error('rp5 reflection');
 "#,
    );
}
#[test]
fn css_text_attribute_and_jquery_writes_cannot_bypass_frozen_fields() {
    let mut engine = fixture();
    run(
        &mut engine,
        r#"
    const a=document.getElementById('a');a.style.width='10px';
    a.style.cssText='width:30px;height:40px';if(a.style.width!=='10px'||a.style.height!=='40px')throw Error('cssText freeze');
    a.setAttribute('style','width:80px;height:90px;overflow:hidden');
    if(a.style.width!=='10px'||a.style.height!=='40px'||a.style.overflow!=='hidden')throw Error('attribute bypass');
    $(a).attr('style','width:99px;display:block');if(a.style.width!=='10px'||a.style.display!=='block')throw Error('jquery bypass');
    a.removeAttribute('style');if(a.style.width!=='10px')throw Error('removal bypass');
    const b=document.getElementById('b');b.setAttribute('style','width:7px');b.setAttribute('style','width:9px');b.style.width='11px';if(b.style.width!=='7px')throw Error('first attribute write');
 "#,
    );
}
#[test]
fn shorthand_and_custom_properties_share_canonical_field_locks() {
    let mut engine = fixture();
    run(
        &mut engine,
        r#"
    const s=document.getElementById('a').style;
    s.margin='1px 2px';s.marginTop='10px';s.setProperty('margin-left','11px');if(s.marginTop!=='1px'||s.marginLeft!=='2px')throw Error('shorthand locks');
    s.setProperty('--Tone','red');s.setProperty('--Tone','blue');s.setProperty('--tone','green');
    if(s.getPropertyValue('--Tone')!=='red'||s.getPropertyValue('--tone')!=='green')throw Error('custom property case');
    if(s.removeProperty('--Tone')!=='red'||s.getPropertyValue('--Tone')!=='red')throw Error('frozen removal');
    const b=document.getElementById('b').style;b.paddingLeft='3px';b.padding='10px';if(b.paddingLeft!=='3px')throw Error('longhand protected');
 "#,
    );
}
#[test]
fn rejected_values_and_mutation_quota_do_not_latch_uncommitted_fields() {
    let mut engine = fixture();
    run(
        &mut engine,
        r#"
    const s=document.getElementById('a').style;
    s.setProperty('height','10px','bad');s.height='12px';if(s.height!=='12px')throw Error('invalid priority latched');
    s.width='10px; height:100px';s.width='17px';if(s.width!=='17px'||s.height!=='12px')throw Error('declaration injection');
    let failed=false;try{s.setProperty('--large','x'.repeat(300000))}catch(e){failed=e.name==='QuotaExceededError'}if(!failed)throw Error('quota');
    s.setProperty('--large','small');if(s.getPropertyValue('--large')!=='small')throw Error('failed mutation latched');
 "#,
    );
}
