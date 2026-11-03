use crate::javascript_dom::{self, WindowMetrics};
use hull_quickjs::Engine;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

fn fixture() -> Engine {
    let mut engine = Engine::new().unwrap();
    engine.install_console().unwrap();
    javascript_dom::install_for_page(
        &mut engine,
        Rc::new(RefCell::new(rp5::parse(
            "<base href='https://fixture.test/base/'><p>stay</p>",
        ))),
        Some(Rc::new(Cell::new(WindowMetrics {
            inner_width: 1280,
            inner_height: 720,
        }))),
        Rc::new(Cell::new(None)),
        "https://fixture.test/path/page",
    )
    .unwrap();
    engine
}
fn run(engine: &mut Engine, source: &str) {
    engine
        .eval(source.as_bytes(), "navigation-test", false)
        .unwrap();
}

#[test]
fn blocked_navigation_logs_three_targets_then_one_warning_without_changing_page() {
    let mut engine = fixture();
    run(
        &mut engine,
        r#"
        const original=document.URL;
        if(location!==document.location||location.href!==original||location.pathname!=='/path/page')throw Error('location reads');
        location.href='next';
        location.assign('/elsewhere');
        if(window.open('javascript:globalThis.bad=1')!==null)throw Error('popup handle');
        location.replace('/silent');
        for(let i=0;i<30;i++){location='/silent';window.open('/silent');location.reload();}
        if(location.href!==original||document.URL!==original||globalThis.bad!==undefined||document.body.textContent!=='stay')throw Error('navigation escaped');
    "#,
    );
    let messages = engine.take_console().messages;
    assert_eq!(messages.len(), 4);
    assert!(messages[0].text.contains("https://fixture.test/base/next"));
    assert!(messages[1].text.contains("https://fixture.test/elsewhere"));
    assert!(messages[2].text.contains("javascript:globalThis.bad=1"));
    assert!(messages[3].text.contains("navigation log exceeded"));
    let mut fresh = fixture();
    run(&mut fresh, "document.location='/fresh'");
    assert_eq!(fresh.take_console().messages.len(), 1);
}

#[test]
fn location_component_writes_and_popup_defaults_are_blocked() {
    let mut engine = fixture();
    run(
        &mut engine,
        r#"
        location.hash='part';location.search='q=1';window.open();
        if(String(location)!=='https://fixture.test/path/page')throw Error('changed URL');
        let failed=false;try{location.assign()}catch(e){failed=e instanceof TypeError}if(!failed)throw Error('required arg');
        failed=false;try{location.href='http://['}catch(e){failed=e.name==='SyntaxError'}if(!failed)throw Error('invalid URL');
    "#,
    );
    let messages = engine.take_console().messages;
    assert_eq!(messages.len(), 3);
    assert!(messages[0].text.contains("/path/page#part"));
    assert!(messages[1].text.contains("/path/page?q=1"));
    assert!(messages[2].text.contains("about:blank"));
}
