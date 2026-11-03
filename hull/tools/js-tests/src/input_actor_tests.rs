use crate::javascript::{EnqueueResult, Report, Runtime};
use crate::javascript_input::from_batch;
use crate::scripts::{ScriptSource, Scripts};
use serde_json::{Value, json};
use winit_trueos::{InputBatch, InputEvent, KeyboardOutputEvent, PointerEvent};

fn sources(body: &str) -> Vec<ScriptSource> {
    let document = rp5::parse(&format!("<script>{body}</script>"));
    let mut scripts = Scripts::default();
    scripts.extract(&document, "https://fixture.test/input");
    scripts.sources
}
fn input(kind: &str) -> Value {
    json!({"kind":kind,"window_id":7,"source":"host-fixture"})
}
const GATE: &str = "https://fixture.test/input-gate.js";
fn blocked(generation: u64, setup: &str) -> Runtime {
    crate::netfs::install([(
        GATE.to_owned(),
        b"globalThis.initializationStage=2".to_vec(),
    )]);
    crate::netfs::delay(GATE, 1000000);
    let mut runtime = Runtime::start().unwrap();
    runtime.enable_resource_loading();
    let doc = rp5::parse(&format!(
        "<script>{setup}</script><script src='{GATE}'></script>"
    ));
    let mut scripts = Scripts::default();
    scripts.extract(&doc, "https://fixture.test/input");
    runtime.replace_page(generation, "https://fixture.test/input", scripts.sources);
    runtime.run().unwrap();
    let reports = until(&mut runtime, |reports| {
        reports
            .iter()
            .any(|r| matches!(r, Report::Evaluated { .. }))
    });
    for report in reports {
        if let Report::Evaluated { result, .. } = report {
            assert!(result.is_ok(), "{result:?}");
        }
    }
    runtime
}
fn turns(runtime: &mut Runtime, count: usize) -> Vec<Report> {
    (0..count)
        .flat_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(1));
            runtime.poll()
        })
        .collect()
}
fn until(runtime: &mut Runtime, predicate: impl Fn(&[Report]) -> bool) -> Vec<Report> {
    let mut reports = Vec::new();
    let waiting = std::time::Instant::now();
    while waiting.elapsed() < std::time::Duration::from_secs(3) {
        std::thread::yield_now();
        let turn = runtime.poll();
        // UI polling drains reports; the worker owns its bounded batches.
        reports.extend(turn);
        if predicate(&reports) {
            return reports;
        }
    }
    panic!("input actor did not complete bounded fixture turns");
}
fn active(generation: u64, source: &str) -> Runtime {
    let mut runtime = Runtime::start().unwrap();
    runtime.replace_page(generation, "https://fixture.test/input", sources(source));
    runtime.run().unwrap();
    let reports = until(&mut runtime, |reports| {
        reports
            .iter()
            .any(|report| matches!(report, Report::Evaluated { .. }))
    });
    for report in reports {
        if let Report::Evaluated { result, .. } = report {
            assert!(result.is_ok(), "registration script failed: {result:?}");
        }
    }
    runtime
}
fn deliveries(reports: &[Report]) -> usize {
    reports
        .iter()
        .filter(|report| matches!(report, Report::Input { .. }))
        .count()
}
fn assert_input_success(reports: &[Report], expected: usize) {
    assert_eq!(deliveries(reports), expected);
    for report in reports {
        if let Report::Input { result, .. } = report {
            assert!(result.is_ok(), "input callback failed: {result:?}");
        }
    }
}

#[test]
fn dormant_input_never_runs_page_or_replays_after_explicit_activation() {
    let mut runtime = Runtime::start().unwrap();
    runtime.replace_page(101,"https://fixture.test/input",sources("if(hullInput.snapshot().received!=='0')throw Error('historic input');hullInput.addEventListener('focus',()=>{if(hullInput.snapshot().received!=='1')throw Error('historic input replay');});"));
    assert!(matches!(
        runtime.enqueue_input(101, input("focus")),
        EnqueueResult::Dormant
    ));
    let idle = turns(&mut runtime, 24);
    assert!(!idle.iter().any(|report| matches!(
        report,
        Report::Evaluated { .. } | Report::Input { .. } | Report::Skipped { .. }
    )));
    runtime.run().unwrap();
    let registered = until(&mut runtime, |reports| {
        reports
            .iter()
            .any(|report| matches!(report, Report::Evaluated { .. }))
    });
    assert!(
        registered
            .iter()
            .any(|report| matches!(report, Report::Evaluated { result: Ok(()), .. }))
    );
    assert_eq!(deliveries(&turns(&mut runtime, 8)), 0);
    assert!(matches!(
        runtime.enqueue_input(101, input("focus")),
        EnqueueResult::Accepted
    ));
    assert_input_success(&until(&mut runtime, |reports| deliveries(reports) == 1), 1);
}

#[test]
fn real_native_pointer_and_keyboard_payloads_reach_business_handlers_in_order() {
    let a = PointerEvent {
        controller_id: u32::MAX,
        slot_id: 0xffff_fffe,
        ep_target: 0x8000_0001,
        hid_kind: 2,
        buttons_down: 0x8000_0005,
        buttons_pressed: 0x8000_0001,
        buttons_released: 2,
        combo_id: 77,
        ..Default::default()
    };
    let b = PointerEvent {
        slot_id: 12,
        buttons_down: 2,
        buttons_pressed: 2,
        buttons_released: 0,
        ..a
    };
    let keyboard = KeyboardOutputEvent {
        controller_id: 4,
        slot_id: 5,
        ep_target: 6,
        modifiers: 0xa5,
        kind: 4,
        key_code: 0x2345,
        utf8_len: 4,
        utf8: [0xf0, 0x9f, 0x98, 0x80],
        flags: 1 << 5,
        ..Default::default()
    };
    let events = from_batch(
        7,
        InputBatch {
            events: vec![
                InputEvent::Pointer(a),
                InputEvent::Pointer(b),
                InputEvent::Keyboard(keyboard),
            ],
            dropped: 0,
        },
    );
    let script = format!(
        r#"
        const expected={expected};let seen=0,microtasks=0;
        hullInput.addEventListener('*',function(event){{
            if(this!==hullInput)throw Error('wrong callback receiver');
            const original=expected[seen];
            if(event.kind!==original.kind||event.window_id!==7||event.device_id!==original.device_id)throw Error('identity/order lost');
            if(JSON.stringify(event.raw)!==JSON.stringify(original.raw)||JSON.stringify(event.endpoint)!==JSON.stringify(original.endpoint))throw Error('native fields lost');
            if(event.generation!=='102'||typeof event.revision!=='string'||typeof event.sequence!=='string')throw Error('epoch metadata lost');
            if(!Object.isFrozen(event)||!Object.isFrozen(event.raw)||!Object.isFrozen(event.endpoint))throw Error('mutable event');
            if(microtasks!==seen)throw Error('input overtook previous callback promises');
            const snapshot=hullInput.snapshot();
            if(!Object.isFrozen(snapshot)||snapshot.received!==String(seen+1)||!Object.isFrozen(snapshot.lastEvent))throw Error('snapshot');
            if(event.kind==='keyboard'&&(event.text!=='😀'||event.raw.modifiers!==165||!Object.isFrozen(event.raw.utf8)))throw Error('keyboard text/modifiers');
            seen++;Promise.resolve().then(()=>microtasks++);
        }});
    "#,
        expected = json!(events)
    );
    let mut runtime = active(102, &script);
    for event in events {
        assert!(matches!(
            runtime.enqueue_input(102, event),
            EnqueueResult::Accepted
        ));
    }
    assert_input_success(&until(&mut runtime, |reports| deliveries(reports) == 3), 3);
}

#[test]
fn navigation_discards_queued_inputs_and_requires_a_fresh_run_request() {
    let mut runtime = active(
        201,
        "globalThis.oldInputPage=true;hullInput.addEventListener('*',()=>{});",
    );
    assert!(matches!(
        runtime.enqueue_input(201, input("focus")),
        EnqueueResult::Accepted
    ));
    runtime.replace_page(202,"https://fixture.test/new",sources("if(typeof oldInputPage!=='undefined')throw Error('old context');hullInput.addEventListener('focus',event=>{if(event.generation!=='202'||hullInput.snapshot().received!=='1')throw Error('old input leaked');});"));
    let inert = turns(&mut runtime, 16);
    assert!(!inert.iter().any(|report| matches!(
        report,
        Report::Input {
            generation: 202,
            ..
        } | Report::Evaluated {
            generation: 202,
            ..
        }
    )));
    assert!(matches!(
        runtime.enqueue_input(202, input("focus")),
        EnqueueResult::Dormant
    ));
    runtime.run().unwrap();
    let registered = until(&mut runtime, |reports| {
        reports
            .iter()
            .any(|report| matches!(report, Report::Evaluated { .. }))
    });
    assert!(
        registered
            .iter()
            .any(|report| matches!(report, Report::Evaluated { result: Ok(()), .. }))
    );
    assert!(matches!(
        runtime.enqueue_input(201, input("focus")),
        EnqueueResult::Stale
    ));
    assert!(matches!(
        runtime.enqueue_input(202, input("focus")),
        EnqueueResult::Accepted
    ));
    assert_input_success(&until(&mut runtime, |reports| deliveries(reports) == 1), 1);
}

#[test]
fn callbacks_continue_after_error_and_promises_finish_before_next_input() {
    let mut runtime = active(
        301,
        "let secondListenerRan=false,promiseRan=false;hullInput.addEventListener('focus',()=>{throw Error('first-listener-broke');});hullInput.addEventListener('focus',()=>{secondListenerRan=true;Promise.resolve().then(()=>promiseRan=true);});hullInput.addEventListener('resize',()=>{if(!secondListenerRan||!promiseRan)throw Error('remaining listeners or promises skipped');});",
    );
    assert!(matches!(
        runtime.enqueue_input(301, input("focus")),
        EnqueueResult::Accepted
    ));
    assert!(matches!(
        runtime.enqueue_input(301, input("resize")),
        EnqueueResult::Accepted
    ));
    let reports = until(&mut runtime, |reports| deliveries(reports) == 2);
    let results: Vec<_> = reports
        .iter()
        .filter_map(|report| match report {
            Report::Input { kind, result, .. } => Some((kind, result)),
            _ => None,
        })
        .collect();
    assert_eq!(results[0].0, "focus");
    assert!(
        results[0]
            .1
            .as_ref()
            .unwrap_err()
            .contains("first-listener-broke")
    );
    assert_eq!(results[1].0, "resize");
    assert!(results[1].1.is_ok());
}

#[test]
fn once_and_removed_listeners_observe_only_the_requested_events() {
    let mut runtime = active(
        302,
        "let onceCount=0,focusCount=0;const removed=()=>{throw Error('removed listener ran');};hullInput.addEventListener('focus',removed);hullInput.removeEventListener('focus',removed);hullInput.addEventListener('focus',()=>onceCount++,{once:true});hullInput.addEventListener('focus',()=>focusCount++);hullInput.addEventListener('resize',()=>{if(onceCount!==1||focusCount!==2)throw Error('listener once/removal semantics');});",
    );
    for kind in ["focus", "focus", "resize"] {
        assert!(matches!(
            runtime.enqueue_input(302, input(kind)),
            EnqueueResult::Accepted
        ));
    }
    assert_input_success(&until(&mut runtime, |reports| deliveries(reports) == 3), 3);
}

#[test]
fn input_api_does_not_connect_to_dom_or_rendering() {
    let mut runtime = active(
        401,
        "for(const name of ['window','document','EventTarget','HTMLElement','fetch','requestAnimationFrame'])if(typeof globalThis[name]!=='undefined')throw Error('DOM or rendering API exposed: '+name);if(!Object.isFrozen(hullInput))throw Error('mutable input API');hullInput.addEventListener('focus',()=>{if(typeof document!=='undefined')throw Error('input created DOM');});",
    );
    assert!(matches!(
        runtime.enqueue_input(401, input("focus")),
        EnqueueResult::Accepted
    ));
    assert_input_success(&until(&mut runtime, |reports| deliveries(reports) == 1), 1);
}

#[test]
fn invalid_or_oversized_input_is_rejected_before_delivery() {
    let mut runtime = active(501, "");
    for event in [
        json!(null),
        json!({"window_id":7}),
        json!({"kind":"focus","window_id":-1}),
        json!({"kind":"focus","window_id":7,"blob":"x".repeat(17000)}),
    ] {
        assert!(matches!(
            runtime.enqueue_input(501, event),
            EnqueueResult::Invalid
        ));
    }
    assert_eq!(deliveries(&turns(&mut runtime, 8)), 0);
}

#[test]
fn queue_overflow_resets_held_state_before_later_input() {
    let mut runtime = blocked(
        601,
        "let reset=false;hullInput.addEventListener('input-reset',event=>{reset=true;if(hullInput.snapshot().resetCount==='0')throw Error('reset count');});hullInput.addEventListener('focus',event=>{if(!reset)throw Error('input arrived before overflow reset');});",
    );
    let mut overflow = 0;
    for index in 0..400 {
        let mut event = input("pointer");
        event["fixture_index"] = json!(index);
        if matches!(runtime.enqueue_input(601, event), EnqueueResult::Overflow) {
            overflow += 1;
        }
    }
    assert!(overflow > 0, "input queue was not bounded");
    crate::netfs::delay(GATE, 0);
    let reset = until(&mut runtime, |reports| {
        reports
            .iter()
            .any(|report| matches!(report,Report::Input{kind,..}if kind=="input-reset"))
    });
    let first = reset
        .iter()
        .find(|r| matches!(r, Report::Input { .. }))
        .unwrap();
    assert!(matches!(first,Report::Input{kind,..} if kind=="input-reset"));
    for report in &reset {
        if let Report::Input { result, .. } = report {
            assert!(result.is_ok(), "{result:?}");
        }
    }
    assert!(matches!(
        runtime.enqueue_input(601, input("focus")),
        EnqueueResult::Accepted
    ));
    let remaining = until(&mut runtime, |reports| {
        reports
            .iter()
            .any(|report| matches!(report,Report::Input{kind,..}if kind=="focus"))
    });
    for report in remaining {
        if let Report::Input { result, .. } = report {
            assert!(result.is_ok(), "post-reset input failed: {result:?}");
        }
    }
}

#[test]
fn input_burst_is_fifo_with_microtasks_between_messages_on_native_worker() {
    let mut runtime = active(
        701,
        "let next=0,microtasks=0;hullInput.addEventListener('pointer',event=>{if(event.fixture_index!==next||microtasks!==next)throw Error('burst reordered or overtook microtasks');next++;Promise.resolve().then(()=>microtasks++);});hullInput.addEventListener('focus',()=>{if(next!==96||microtasks!==96)throw Error('burst incomplete');});",
    );
    for index in 0..96 {
        let mut event = input("pointer");
        event["fixture_index"] = json!(index);
        assert!(matches!(
            runtime.enqueue_input(701, event),
            EnqueueResult::Accepted
        ));
    }
    assert!(matches!(
        runtime.enqueue_input(701, input("focus")),
        EnqueueResult::Accepted
    ));
    let all = until(&mut runtime, |reports| deliveries(reports) >= 97);
    assert_input_success(&all, 97);
}

#[test]
fn page_override_of_object_is_frozen_cannot_disable_payload_immutability() {
    let mut runtime = active(
        702,
        "'use strict';const genuineFrozen=Object.isFrozen;Object.isFrozen=()=>true;hullInput.addEventListener('focus',event=>{if(!genuineFrozen(event)||!genuineFrozen(event.raw)||!genuineFrozen(event.raw.nested)||!genuineFrozen(event.raw.nested.list))throw Error('page bypassed readonly delivery');try{event.raw.nested.list[0]=99;}catch{}if(event.raw.nested.list[0]!==1)throw Error('nested value mutated');const snapshot=hullInput.snapshot();if(!genuineFrozen(snapshot)||!genuineFrozen(snapshot.lastEvent))throw Error('mutable snapshot');});",
    );
    let mut event = input("focus");
    event["raw"] = json!({"nested":{"list":[1,2]}});
    assert!(matches!(
        runtime.enqueue_input(702, event),
        EnqueueResult::Accepted
    ));
    assert_input_success(&until(&mut runtime, |reports| deliveries(reports) == 1), 1);
}

#[test]
fn repeated_overflows_during_initialization_accumulate_a_global_reset() {
    let mut runtime = blocked(
        703,
        "let dropped=0,received=0,resetMessages=0;function balance(){if(dropped+received===800)console.log('balanced');}hullInput.addEventListener('input-reset',event=>{if(event.window_id!==null||event.scope!=='all-windows'||event.reset_all!==true)throw Error('ambiguous reset scope');dropped+=Number(event.dropped);resetMessages++;balance();});hullInput.addEventListener('pointer',()=>{if(!resetMessages)throw Error('input overtook reset');received++;balance();});hullInput.addEventListener('focus',()=>{if(dropped+received!==800||resetMessages!==1||initializationStage!==2)throw Error('pending overflow loss was overwritten: '+dropped+'/'+received+'/'+resetMessages);});",
    );
    let mut overflow_count = 0;
    for _ in 0..400 {
        if matches!(
            runtime.enqueue_input(703, input("pointer")),
            EnqueueResult::Overflow
        ) {
            overflow_count += 1;
        }
    }
    let mut reports = runtime.poll();
    assert_eq!(
        deliveries(&reports),
        0,
        "input ran before initial source batch"
    );
    for _ in 0..400 {
        if matches!(
            runtime.enqueue_input(703, input("pointer")),
            EnqueueResult::Overflow
        ) {
            overflow_count += 1;
        }
    }
    reports.extend(runtime.poll());
    assert!(
        overflow_count > 0,
        "blocked initial source must bound input queue"
    );
    assert_eq!(
        deliveries(&reports),
        0,
        "input ran while initial script resource pending"
    );
    crate::netfs::delay(GATE, 0);
    let balanced = until(&mut runtime, |reports| {
        reports.iter().any(|r|matches!(r,Report::Console{batch,..} if batch.messages.iter().any(|m|m.text=="balanced")))
    });
    assert!(
        matches!(balanced.iter().find(|r|matches!(r,Report::Input{..})).unwrap(),Report::Input{kind,..} if kind=="input-reset")
    );
    for report in &balanced {
        if let Report::Input { result, .. } = report {
            assert!(result.is_ok(), "{result:?}");
        }
    }

    assert!(matches!(
        runtime.enqueue_input(703, input("focus")),
        EnqueueResult::Accepted
    ));
    let ready = until(&mut runtime, |reports| {
        reports
            .iter()
            .any(|report| matches!(report,Report::Input{kind,..}if kind=="focus"))
    });
    for report in &ready {
        if let Report::Input { result, .. } = report {
            assert!(result.is_ok(), "{result:?}");
        }
    }
}
