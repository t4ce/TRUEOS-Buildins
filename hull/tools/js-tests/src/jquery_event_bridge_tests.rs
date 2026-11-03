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
