use crate::javascript_dom::{self, WindowMetrics};
use hull_quickjs::Engine;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
fn fixture(window: bool) -> Engine {
    let mut engine = Engine::new().unwrap();
    engine.install_console().unwrap();
    let doc = Rc::new(RefCell::new(rp5::parse("<main><p>before</p></main>")));
    if window {
        javascript_dom::install_with_window(
            &mut engine,
            doc,
            Rc::new(Cell::new(WindowMetrics {
                inner_width: 1280,
                inner_height: 720,
            })),
        )
        .unwrap();
    } else {
        javascript_dom::install(&mut engine, doc).unwrap();
    }
    engine
}
fn run(engine: &mut Engine, source: &str) {
    engine
        .eval(source.as_bytes(), "dom-events-fixture.js", false)
        .unwrap();
}
#[test]
fn event_targets_capture_and_bubble_on_real_node_ancestry() {
    let mut engine = fixture(true);
    run(
        &mut engine,
        r#"
    const p=document.querySelector('p'),main=p.parentNode,trace=[];
    if(!(p instanceof EventTarget)||!(document instanceof EventTarget))throw Error('EventTarget inheritance');
    window.addEventListener('test',e=>{if(e.eventPhase!==Event.CAPTURING_PHASE)throw Error('capture phase');trace.push('window-capture')},true);
    document.addEventListener('test',e=>trace.push('document-capture'),true);
    main.addEventListener('test',e=>trace.push('main-capture'),true);
    p.addEventListener('test',function(e){if(this!==p||e.target!==p||e.currentTarget!==p||e.eventPhase!==Event.AT_TARGET||e.isTrusted)throw Error('target');trace.push('target-capture')},true);
    p.addEventListener('test',e=>trace.push('target-bubble'));
    main.addEventListener('test',e=>trace.push('main-bubble'));
    document.addEventListener('test',e=>trace.push('document-bubble'));
    window.addEventListener('test',e=>{if(e.eventPhase!==Event.BUBBLING_PHASE||e.composedPath().at(-1)!==window)throw Error('bubble phase');trace.push('window-bubble')});
    const e=new CustomEvent('test',{bubbles:true,cancelable:true,detail:{answer:42}});
    if(!p.dispatchEvent(e)||e.detail.answer!==42||e.currentTarget!==null||e.eventPhase!==Event.NONE||e.composedPath().length)throw Error('cleanup/detail');
    if(trace.join(',')!=='window-capture,document-capture,main-capture,target-capture,target-bubble,main-bubble,document-bubble,window-bubble')throw Error(trace);
    if(typeof __hullInputReceive!=='undefined')throw Error('unpinned receiver');
"#,
    );
}
#[test]
fn listener_identity_once_removal_objects_and_reentrancy() {
    let mut engine = fixture(false);
    run(
        &mut engine,
        r#"
    const t=new EventTarget(),trace=[],removed=()=>trace.push('removed');
    function repeated(){trace.push('repeated');t.removeEventListener('x',removed);t.addEventListener('x',()=>trace.push('late'),{once:true})}
    t.addEventListener('x',repeated);t.addEventListener('x',repeated,{once:true});t.addEventListener('x',removed);
    const object={handleEvent(e){if(this!==object||e.currentTarget!==t)throw Error('handleEvent receiver');trace.push('object')}};
    t.addEventListener('x',object,{once:true});t.addEventListener('x',null);
    t.dispatchEvent(new Event('x'));if(trace.join(',')!=='repeated,object')throw Error(trace);
    trace.length=0;t.dispatchEvent(new Event('x'));if(trace.join(',')!=='repeated,late')throw Error(trace);
    let n=0;t.addEventListener('once',()=>{n++;t.dispatchEvent(new Event('once'))},{once:true});t.dispatchEvent(new Event('once'));if(n!==1)throw Error('reentrant once');
    t.addEventListener('reentrant',e=>{try{t.dispatchEvent(e);throw Error('accepted redispatch')}catch(x){if(x.name!=='InvalidStateError')throw x}});
    t.dispatchEvent(new Event('reentrant'));
    let empty=0;t.addEventListener('',()=>empty++);t.dispatchEvent(new Event(''));if(empty!==1)throw Error('empty type');
    let preStopped=0;t.addEventListener('pre-stopped',()=>preStopped++);const stopped=new Event('pre-stopped');stopped.stopPropagation();t.dispatchEvent(stopped);if(preStopped)throw Error('pre-dispatch stop');
"#,
    );
    assert!(engine.take_console().messages.is_empty());
}
#[test]
fn cancellation_passive_stop_propagation_and_exception_reporting() {
    let mut engine = fixture(false);
    run(
        &mut engine,
        r#"
    const p=document.querySelector('p'),main=p.parentNode,trace=[];
    main.addEventListener('x',()=>trace.push('parent'));
    p.addEventListener('x',e=>{e.preventDefault();e.stopPropagation();trace.push('first')});
    p.addEventListener('x',()=>trace.push('second'));
    const e=new Event('x',{bubbles:true,cancelable:true});if(p.dispatchEvent(e)!==false||!e.defaultPrevented||trace.join(',')!=='first,second')throw Error('cancel/propagate');
    p.addEventListener('passive',e=>e.preventDefault(),{passive:true});if(!p.dispatchEvent(new Event('passive',{cancelable:true})))throw Error('passive canceled');
    document.addEventListener('wheel',e=>e.preventDefault());if(!document.dispatchEvent(new Event('wheel',{cancelable:true})))throw Error('default passive');
    document.addEventListener('wheel',e=>e.preventDefault(),{passive:false});if(document.dispatchEvent(new Event('wheel',{cancelable:true})))throw Error('explicit nonpassive');
    const t=new EventTarget();let count=0;t.addEventListener('stop',e=>e.stopImmediatePropagation());t.addEventListener('stop',()=>count++);t.dispatchEvent(new Event('stop'));if(count)throw Error('immediate');
    t.addEventListener('error-test',()=>{throw Error('listener-reported')});t.addEventListener('error-test',()=>count++);if(!t.dispatchEvent(new Event('error-test'))||count!==1)throw Error('exception stopped dispatch');
"#,
    );
    let batch = engine.take_console();
    assert!(
        batch
            .messages
            .iter()
            .any(|r| r.text.contains("listener-reported"))
    );
}
#[test]
fn event_path_is_fixed_even_if_a_listener_detaches_target() {
    let mut engine = fixture(false);
    run(
        &mut engine,
        r#"
    const p=document.querySelector('p'),main=p.parentNode;let seen=false;
    p.addEventListener('x',()=>{main.textContent='detached'});
    main.addEventListener('x',e=>{if(e.target!==p||p.parentNode!==null)throw Error('path');seen=true});
    p.dispatchEvent(new Event('x',{bubbles:true}));if(!seen)throw Error('path changed');
"#,
    );
    assert!(engine.take_console().messages.is_empty());
}

#[test]
fn actual_winit_window_records_dispatch_dom_window_events_and_keep_raw_delivery() {
    let mut engine = fixture(true);
    run(
        &mut engine,
        r#"
        let windowEvents=[];
        for(const kind of ['focus','blur','resize'])window.addEventListener(kind,function(event){
            if(this!==window||event.target!==window||!event.isTrusted)throw Error('native window event identity');
            windowEvents.push(event.type);
        });
    "#,
    );
    for record in [
        r#"{"kind":"focus","source":"winit-window-state","focused":true}"#,
        r#"{"kind":"resize","source":"winit-window-state","width":1280,"height":752}"#,
        r#"{"kind":"focus","source":"winit-window-state","focused":false}"#,
    ] {
        engine.dispatch_json(record.as_bytes()).unwrap();
    }
    run(
        &mut engine,
        "if(windowEvents.join(',')!=='focus,resize,blur'||hullInput.snapshot().received!=='3')throw Error('native/raw routing')",
    );
    assert!(engine.take_console().messages.is_empty());
}
