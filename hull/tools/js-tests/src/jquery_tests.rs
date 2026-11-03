//! Guest-to-native production binding, sharing the same rp5 arena and quotas.
use crate::javascript_dom::{self, SharedDocument};
use hull_quickjs::Engine;
use std::{cell::RefCell, rc::Rc};
fn fixture() -> (Engine, SharedDocument) {
    let doc = Rc::new(RefCell::new(rp5::parse(
        "<main id='box'><p class='note'>one</p><section><p class='note'>two</p></section></main>",
    )));
    let mut engine = Engine::new().unwrap();
    javascript_dom::install(&mut engine, doc.clone()).unwrap();
    (engine, doc)
}
fn run(engine: &mut Engine, source: &str) {
    engine
        .eval(source.as_bytes(), "native-jquery-fixture.js", false)
        .unwrap();
}
#[test]
fn native_collections_keep_dom_identity_order_and_stack() {
    let (mut engine, _) = fixture();
    run(
        &mut engine,
        r#"
    if($!==jQuery||typeof $.fn.jquery!=='string')throw Error('globals');
    const notes=$('p.note'),first=document.querySelector('p');
    if(notes.length!==2||notes[0]!==first||notes.get(0)!==first||[...notes][0]!==first)throw Error('identity');
    if(notes.eq(-1).text()!=='two'||notes.first().text()!=='one'||notes.last().text()!=='two')throw Error('selection');
    if(notes.eq(1).end()!==notes||notes.slice(-1).text()!=='two'||notes.slice(0,0).length!==0)throw Error('stack');
    if(notes.even().text()!=='one'||notes.odd().text()!=='two'||notes.eq(NaN).length||$('').length||$(false).length)throw Error('indexes');
    try{notes.eq(0.5);throw Error('accepted fractional index')}catch(e){if(e.name!=='NotSupportedError')throw e}
    if($('#box').find('p').text()!=='onetwo'||$('p',$('#box')).length!==2||notes.find('').length)throw Error('find');
    if(notes.first().parent()[0]!==document.querySelector('main')||notes.last().parents().first()[0]!==document.querySelector('section'))throw Error('traversal');
    if($('#box').children().length!==2||notes.first().next()[0]!==document.querySelector('section'))throw Error('siblings');
    let count=0;notes.each(function(i,node){if(this!==node||node!==notes[i])throw Error('callback');count++;return false});if(count!==1)throw Error('each');
    notes[0]=notes[1];if(notes.first().text()!=='two')throw Error('mutable collection indices');
    if(typeof __hullDom!=='undefined'||typeof fetch!=='undefined'||typeof require!=='undefined')throw Error('unexpected capability');
"#,
    );
}
#[test]
fn native_mutations_reach_actual_rp5_and_callbacks_run_in_guest() {
    let (mut engine, doc) = fixture();
    run(
        &mut engine,
        r#"
    const notes=$('p');if(notes.addClass('a b').removeClass('note').toggleClass('b c')!==notes)throw Error('chain');
    if(!notes.hasClass('a')||notes.hasClass('b')||!notes.hasClass('c'))throw Error('classes');
    notes.attr({'data-n':5,'aria-hidden':false,'hidden':false});
    if(notes.attr('data-n')!=='5'||notes.attr('aria-hidden')!=='false'||notes.attr('hidden')!==undefined)throw Error('attrs');
    notes.attr('data-n',function(i,old){return +old+i});
    notes.text(function(i,old){return `${i}:${old}`});
    if(notes.text()!=='0:one1:two'||notes.last().attr('data-n')!=='6')throw Error('callback setters');
    notes.removeAttr('data-n aria-hidden');notes.removeClass();notes.addClass(undefined);
    if(notes.attr('class')!==''||notes.attr('data-n')!==undefined)throw Error('remove');
    if(document.querySelector('p').textContent!=='0:one')throw Error('shared tree');
"#,
    );
    let document = doc.borrow();
    let ids = document
        .nodes
        .iter()
        .enumerate()
        .filter(|(i, _)| document.name(*i) == "p")
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    assert_eq!(document.text_content(ids[0]), "0:one");
    assert_eq!(document.text_content(ids[1]), "1:two");
}
#[test]
fn native_utilities_preserve_undefined_numbers_and_encoding_without_to_json() {
    let (mut engine, _) = fixture();
    run(
        &mut engine,
        r#"
    if($.param({a:[1,2],b:null,c:undefined,n:NaN,i:Infinity})!=='a%5B%5D=1&a%5B%5D=2&b=&c=&n=NaN&i=Infinity')throw Error('param data');
    if($.param({a:[1,2]},true)!=='a=1&a=2'||$.param([{name:'q',value:'é a'}])!=='q=%C3%A9%20a')throw Error('param modes');
    if($.escapeSelector('0x #')!=='\\30 x\\ \\#')throw Error('escape');
    Array.prototype.toJSON=()=>{throw Error('transport prototype hook')};
    Object.prototype.toJSON=()=>{throw Error('object prototype hook')};
    if($('p').length!==2||$.param({q:'safe'})!=='q=safe')throw Error('transport');
    delete Array.prototype.toJSON;delete Object.prototype.toJSON;
    const cyclic={};cyclic.x=cyclic;
    try{$.param(cyclic);throw Error('accepted cycle')}catch(e){if(!(e instanceof TypeError))throw e}
    try{$.param({f:()=>1});throw Error('accepted callback')}catch(e){if(e.name!=='NotSupportedError')throw e}
    try{$('<p>');throw Error('accepted HTML')}catch(e){if(e.name!=='NotSupportedError')throw e}
    try{$(()=>{});throw Error('accepted ready')}catch(e){if(e.name!=='NotSupportedError')throw e}
    try{$('p').children('a');throw Error('accepted filtered traversal')}catch(e){if(e.name!=='NotSupportedError')throw e}
    try{$('[');throw Error('accepted invalid selector')}catch(e){if(e.name!=='SyntaxError')throw e}
"#,
    );
}
#[test]
fn native_mutations_share_dom_quotas_and_fail_with_dom_exception() {
    let (mut engine, _) = fixture();
    run(
        &mut engine,
        r#"
    try{$('p').text('x'.repeat(262145));throw Error('accepted oversized mutation')}catch(e){if(e.name!=='QuotaExceededError')throw e}
    try{$('p').attr('',1);throw Error('accepted invalid attribute')}catch(e){if(e.name!=='InvalidCharacterError')throw e}
    if($('p').text()!=='onetwo')throw Error('failed oversized mutation changed tree');
    const quotaChunk='x'.repeat(16384);let quotaStopped=false;
"#,
    );
    // Separate turns exercise the cumulative DOM quota without deliberately
    // exhausting the per-evaluation QuickJS time slice during transport encoding.
    for _ in 0..140 {
        run(
            &mut engine,
            "try{$('p').attr('title',quotaChunk)}catch(e){if(e.name!=='QuotaExceededError')throw e;quotaStopped=true}",
        );
    }
    run(
        &mut engine,
        r#"
    if(!quotaStopped)throw Error('aggregate quota bypass');
    try{document.querySelector('p').setAttribute('title',quotaChunk);throw Error('counter fork')}catch(e){if(e.name!=='QuotaExceededError')throw e}
"#,
    );
}
#[test]
fn native_install_is_per_realm_and_no_conflict_releases_globals() {
    let (mut engine, _) = fixture();
    run(
        &mut engine,
        r#"const local=jQuery.noConflict(true);if(typeof $!=='undefined'||typeof jQuery!=='undefined'||local('p').length!==2)throw Error('noConflict')"#,
    );
    let (mut other, _) = fixture();
    run(
        &mut other,
        "if($('p').text()!=='onetwo')throw Error('realm isolation')",
    );
}

#[test]
fn native_inner_loops_reject_excessive_class_and_object_work_before_mutation() {
    let (mut engine, _) = fixture();
    run(
        &mut engine,
        r#"
        const many=Array.from({length:257},(_,i)=>'c'+i).join(' ');
        try{$('p').addClass(many);throw Error('unbounded classes')}catch(e){if(e.name!=='QuotaExceededError')throw e}
        if($('p').attr('class')!=='note')throw Error('class quota mutated target');
        const wide={};for(let i=0;i<129;i++)wide['k'+i]=i;
        try{$.param(wide);throw Error('unbounded object')}catch(e){if(e.name!=='QuotaExceededError')throw e}
        document.querySelector('p').setAttribute('class',many);
        try{$('p').toggleClass('x');throw Error('unbounded existing classes')}catch(e){if(e.name!=='QuotaExceededError')throw e}
        if(document.querySelector('p').getAttribute('class')!==many)throw Error('old class list changed');
    "#,
    );
}
