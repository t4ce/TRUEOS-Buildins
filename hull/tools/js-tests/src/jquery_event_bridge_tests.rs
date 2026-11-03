use crate::javascript_dom;
use hull_quickjs::Engine;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

fn fixture() -> Engine {
    let mut engine = Engine::new().unwrap();
    engine.install_console().unwrap();
    javascript_dom::install_with_window(
        &mut engine,
        Rc::new(RefCell::new(rp5::parse("<main><button></button></main>"))),
        Rc::new(Cell::new(javascript_dom::WindowMetrics {
            inner_width: 1280,
            inner_height: 720,
        })),
    )
    .unwrap();
    engine
}

fn run(engine: &mut Engine, source: &str) {
    engine
        .eval(source.as_bytes(), "jquery-event-bridge.js", false)
        .unwrap();
}

#[test]
fn jquery_on_one_off_and_mouse_key_aliases_use_real_dom_events() {
    let mut engine = fixture();
    run(
        &mut engine,
        r#"
        const button=document.querySelector('button'), main=button.parentNode;
        const $button=jQuery(button), trace=[], event=new Event('mousemove',{bubbles:true,cancelable:true});
        let oneCount=0;
        const handler=function(e){
            if(this!==button||e!==event||e.data.answer!==42)throw Error('callback target, identity, or data');
            trace.push('handler');return false;
        };
        main.addEventListener('mousemove',()=>trace.push('parent'));
        $button.on('mousemove',{answer:42},handler);
        $button.one('keypress',function(e){if(this!==button)throw Error('one target');oneCount++});
        $button[0].dispatchEvent(event);
        if(!event.defaultPrevented||trace.join(',')!=='handler')throw Error('false return semantics: '+trace);
        $button[0].dispatchEvent(new Event('keypress'));
        $button[0].dispatchEvent(new Event('keypress'));
        if(oneCount!==1)throw Error('one callback count');
        $button.off('mousemove',handler);
        $button[0].dispatchEvent(new Event('mousemove'));
        if(trace.join(',')!=='handler')throw Error('off callback identity');
        let alias=0;$button.mousemove(()=>alias++).keypress(()=>alias++);
        $button[0].dispatchEvent(new Event('mousemove'));$button[0].dispatchEvent(new Event('keypress'));
        if(alias!==2)throw Error('event aliases');
        jQuery().mousemove(()=>{}).keypress(()=>{});
        try{$button.on('mousemove.ns',handler);throw Error('namespace accepted')}catch(e){if(e.name!=='NotSupportedError')throw e}
        try{$button.on('mousemove','button',handler);throw Error('delegation accepted')}catch(e){if(e.name!=='NotSupportedError')throw e}
        "#,
    );
    assert!(engine.take_console().messages.is_empty());
}

#[test]
fn jquery_window_binding_receives_existing_winit_window_event() {
    let mut engine = fixture();
    run(
        &mut engine,
        "let resized=0;jQuery(window).on('resize',function(e){if(this!==window||e.target!==window||!e.isTrusted)throw Error('window event identity');resized++});",
    );
    engine
        .dispatch_json(br#"{"kind":"resize","source":"winit-window-state","width":1280,"height":752}"#)
        .unwrap();
    run(&mut engine, "if(resized!==1)throw Error('window resize not delivered')");
    assert!(engine.take_console().messages.is_empty());
}

#[test]
fn jquery_dispatch_snapshots_handlers_and_cleans_once_before_reentrancy() {
    let mut engine = fixture();
    run(
        &mut engine,
        r#"
        const button=document.querySelector('button'), $button=jQuery(button), trace=[];
        const b=()=>trace.push('b'), c=()=>trace.push('c');
        $button.on('change',function(){trace.push('a');$button.off('change',b);$button.on('change',c)});
        $button.on('change',b);
        button.dispatchEvent(new Event('change'));
        if(trace.join(',')!=='a,b')throw Error('current dispatch must keep its snapshot: '+trace);
        trace.length=0;button.dispatchEvent(new Event('change'));
        if(trace.join(',')!=='a,c')throw Error('later dispatch must see registry changes: '+trace);

        let onceCount=0;
        $button.one('reentrant',function(){onceCount++;button.dispatchEvent(new Event('reentrant'))});
        button.dispatchEvent(new Event('reentrant'));
        if(onceCount!==1)throw Error('one must detach before a reentrant dispatch');

        let duplicateOnceCount=0;
        function sameOnce(){duplicateOnceCount++;button.dispatchEvent(new Event('same-once'))}
        $button.one('same-once',sameOnce).one('same-once',sameOnce);
        button.dispatchEvent(new Event('same-once'));
        if(duplicateOnceCount!==2)throw Error('one must remove all matching handler identities from the live registry');
        button.dispatchEvent(new Event('same-once'));
        if(duplicateOnceCount!==2)throw Error('matching one-shot handlers remained registered');

        let sawUndefinedData=false;
        $button.on('undefined-data',e=>{sawUndefinedData=e.data===undefined});
        const undefinedDataEvent=new Event('undefined-data');Object.defineProperty(undefinedDataEvent,'data',{value:'native-data',configurable:true});
        button.dispatchEvent(undefinedDataEvent);
        if(!sawUndefinedData||undefinedDataEvent.data!=='native-data')throw Error('undefined jQuery data must mask and restore native data');

        const duplicate=()=>trace.push('duplicate');
        $button.on('duplicate',duplicate).on('duplicate',duplicate);
        button.dispatchEvent(new Event('duplicate'));
        if(trace.filter(value=>value==='duplicate').length!==2)throw Error('duplicate handlers');
        $button.off('duplicate',duplicate);trace.length=0;button.dispatchEvent(new Event('duplicate'));
        if(trace.length)throw Error('off must remove every duplicate handler');

        $button.on('halt',false);
        const halted=new Event('halt',{bubbles:true,cancelable:true});button.dispatchEvent(halted);
        if(!halted.defaultPrevented)throw Error('false callback shorthand');
        $button.on('immediate',e=>{trace.push('first');e.stopImmediatePropagation()});
        $button.on('immediate',()=>trace.push('second'));
        trace.length=0;button.dispatchEvent(new Event('immediate'));
        if(trace.join(',')!=='first')throw Error('stopImmediatePropagation');
        trace.length=0;$button.off('');
        for(const type of ['change','reentrant','halt','immediate'])button.dispatchEvent(new Event(type));
        if(trace.length)throw Error('empty off did not clear all event handlers');
        $button.on('cleanup',()=>trace.push('cleanup')).off(' \t ');
        button.dispatchEvent(new Event('cleanup'));
        if(trace.length)throw Error('whitespace off did not clear all event handlers');
        "#,
    );
    assert!(engine.take_console().messages.is_empty());
}

#[test]
fn jquery_document_binding_receives_existing_dom_ready_event() {
    let mut engine = fixture();
    run(
        &mut engine,
        "let ready=0;jQuery(document).one('DOMContentLoaded',function(e){if(this!==document||e.target!==document||!e.isTrusted)throw Error('document ready identity');ready++});",
    );
    engine
        .dispatch_json(br#"{"kind":"__hull_dom_ready"}"#)
        .unwrap();
    run(&mut engine, "if(ready!==1||document.readyState!=='interactive')throw Error('DOM ready not delivered')");
    assert!(engine.take_console().messages.is_empty());
}

#[test]
fn jquery_consumes_html_collection_as_identity_preserving_snapshot() {
    let mut engine = fixture();
    run(
        &mut engine,
        r#"
        const live=document.getElementsByTagName('button'), node=live[0], $snapshot=jQuery(live);
        if($snapshot.length!==1||$snapshot[0]!==node||$snapshot.constructor!==jQuery)throw Error('HTMLCollection conversion');
        document.querySelector('main').textContent='';
        if(live.length!==0||$snapshot.length!==1||$snapshot[0]!==node||node.parentNode!==null)throw Error('jQuery collection must snapshot handles');
        "#,
    );
    assert!(engine.take_console().messages.is_empty());
}

#[test]
fn jquery_ready_shorthand_and_method_share_async_ready_signal() {
    let mut engine = fixture();
    run(&mut engine, r#"
        let trace=[];
        const collection=$(function(jq){
            if(this!==document||jq!==jQuery)throw Error('ready arguments');
            document.querySelector('button').textContent='ready';trace.push('early');
        });
        if(collection[0]!==document||collection.length!==1)throw Error('ready return');
        $(document).ready(()=>{throw Error('ready isolated failure')});
        $(document).ready(()=>trace.push('second'));
        if(trace.length)throw Error('ready ran before signal');
    "#);
    engine.dispatch_json(br#"{"kind":"__hull_dom_ready"}"#).unwrap();
    run(&mut engine, "if(trace.length)throw Error('ready must be async')");
    while engine.execute_pending_job().unwrap() {}
    run(&mut engine, r#"
        if(trace.join(',')!=='early,second'||document.querySelector('button').textContent!=='ready')throw Error('ready delivery');
        $(()=>trace.push('late'));
        if(trace.length!==2)throw Error('late ready must be async');
    "#);
    engine.dispatch_json(br#"{"kind":"__hull_dom_ready"}"#).unwrap();
    while engine.execute_pending_job().unwrap() {}
    run(&mut engine, "if(trace.join(',')!=='early,second,late')throw Error('ready duplicate or lost')");
    let output=engine.take_console();
    assert_eq!(output.messages.len(),1);
    assert!(output.messages[0].text.contains("ready isolated failure"));
}

#[test]
fn detached_html_documents_have_own_roots_and_ten_document_cap() {
    let mut engine = fixture();
    run(&mut engine, r#"
        const second=document.implementation.createHTMLDocument('a <title> & text');
        const third=second.implementation.createHTMLDocument();
        if(!(second instanceof Document)||!(second.implementation instanceof DOMImplementation))throw Error('document brand');
        if(second.implementation!==second.implementation||second.ownerDocument!==null||second.parentNode!==null)throw Error('document identity');
        if(second.defaultView!==null||second.currentScript!==null||second.readyState!=='complete'||second.URL!=='about:blank')throw Error('inert document');
        if(second.documentElement.ownerDocument!==second||second.body.parentNode!==second.documentElement||second.head.ownerDocument!==second)throw Error('tree ownership');
        if(second.querySelector('title').textContent!=='a <title> & text'||third.querySelector('title')!==null)throw Error('title text');
        const node=second.createElement('aside');node.setAttribute('id','inert-only');second.body.appendChild(node);
        if(second.getElementById('inert-only')!==node||document.getElementById('inert-only')!==null)throw Error('document query isolation');
        const child=second.createTextNode('text');node.appendChild(child);
        const copy=node.cloneNode(true);
        if(copy.ownerDocument!==second||copy.firstChild.ownerDocument!==second)throw Error('clone ownership');
        document.body.appendChild(node);
        if(node.ownerDocument!==document||child.ownerDocument!==document||second.getElementById('inert-only')!==null)throw Error('adoption');
        second.body.textContent='new';
        if(second.body.firstChild.ownerDocument!==second)throw Error('text owner');
        const fragment=third.createDocumentFragment();fragment.appendChild(copy);document.body.appendChild(fragment);
        if(copy.ownerDocument!==document||copy.firstChild.ownerDocument!==document||fragment.ownerDocument!==third)throw Error('fragment adoption');
        let windowEvents=0;window.addEventListener('inert-event',()=>windowEvents++);
        second.dispatchEvent(new Event('inert-event',{bubbles:true}));
        if(windowEvents)throw Error('inert events reached window');
        for(let i=3;i<10;i++) third.implementation.createHTMLDocument();
        for(let i=0;i<5;i++) {
            try{third.implementation.createHTMLDocument();throw Error('cap bypass')}
            catch(e){if(e.name!=='QuotaExceededError')throw e}
        }
        if(document.querySelector('main')===null||third.body.ownerDocument!==third)throw Error('cap corrupted existing document');
    "#);
    let output=engine.take_console();
    assert_eq!(output.messages.len(),1);
    assert!(output.messages[0].text.contains("document soft cap exceeded"));
}
