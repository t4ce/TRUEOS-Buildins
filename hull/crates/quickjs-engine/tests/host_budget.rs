use hull_quickjs::{Engine, Limits};
use std::{cell::Cell, rc::Rc, time::Duration};

#[test]
fn native_host_json_round_trips_data_and_survives_hidden_global() {
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    let mut engine = Engine::new().unwrap();
    engine
        .bind_host_handler("bridge", move |json| {
            observed.set(observed.get() + 1);
            Ok(json.to_vec())
        })
        .unwrap();
    engine
        .eval(
            br#"
        const native = bridge;
        delete globalThis.bridge;
        JSON.stringify = () => { throw Error('guest stringify used'); };
        const answer = native({text:'"; globalThis.injected = true; //', array:[1,true,null]});
        if (answer.array[2] !== null || typeof injected !== 'undefined') throw Error('not data');
        if (Object.hasOwn(globalThis, 'bridge')) throw Error('global remained');
    "#,
            "roundtrip.js",
            false,
        )
        .unwrap();
    assert_eq!(calls.get(), 1);
    assert!(
        engine
            .bind_host_handler("another", |_| Ok(b"null".to_vec()))
            .is_err()
    );
}

#[test]
fn host_transport_errors_bad_json_and_panics_do_not_poison_later_calls() {
    let mut engine = Engine::new().unwrap();
    engine
        .bind_host_handler("bridge", |json| match json {
            b"1" => Err("deliberate failure".into()),
            b"2" => Ok(b"not JSON".to_vec()),
            b"3" => panic!("caught host panic"),
            _ => Ok(b"42".to_vec()),
        })
        .unwrap();
    for (source, expected) in [
        (b"bridge(1)".as_slice(), "deliberate failure"),
        (b"bridge(2)".as_slice(), "SyntaxError"),
        (b"bridge(3)".as_slice(), "host handler panicked"),
        (
            b"bridge({toJSON(){while(true){}}})".as_slice(),
            "interrupted",
        ),
    ] {
        let error = engine.eval(source, "error.js", false).unwrap_err();
        assert!(error.contains(expected), "{error}");
        engine
            .eval(
                b"if(bridge(0)!==42)throw Error('not recovered')",
                "recovery.js",
                false,
            )
            .unwrap();
    }
}

#[test]
fn host_transport_caps_both_directions_without_invoking_oversized_request() {
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    let mut engine = Engine::new().unwrap();
    engine
        .bind_host_handler("bridge", move |_| {
            observed.set(observed.get() + 1);
            Ok(vec![b' '; 1024 * 1024 + 1])
        })
        .unwrap();
    assert!(
        engine
            .eval(b"bridge('x'.repeat(1048576))", "large.js", false)
            .unwrap_err()
            .contains("request exceeds")
    );
    assert_eq!(calls.get(), 0);
    assert!(
        engine
            .eval(b"bridge(0)", "large.js", false)
            .unwrap_err()
            .contains("response exceeds")
    );
    assert_eq!(calls.get(), 1);
    for source in [
        b"bridge()".as_slice(),
        b"bridge(undefined)",
        b"const a={};a.a=a;bridge(a)",
    ] {
        assert!(engine.eval(source, "invalid.js", false).is_err());
    }
    assert_eq!(calls.get(), 1);
}

#[test]
fn host_storage_follows_engine_moves_and_drops_with_context() {
    struct DropMarker(Rc<Cell<bool>>);
    impl Drop for DropMarker {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    let dropped = Rc::new(Cell::new(false));
    let marker = DropMarker(dropped.clone());
    let mut engine = Engine::new().unwrap();
    engine
        .bind_host_handler("bridge", move |_| {
            let _keep_alive = &marker;
            Ok(b"null".to_vec())
        })
        .unwrap();
    let mut engines = vec![engine];
    engines[0].eval(b"bridge(0)", "moved.js", false).unwrap();
    assert!(!dropped.get());
    drop(engines);
    assert!(dropped.get());
}

#[test]
fn one_active_budget_covers_multiple_calls_but_excludes_idle_waits() {
    let mut engine = Engine::new().unwrap();
    engine
        .bind_host_handler("bridge", |_| {
            std::thread::sleep(Duration::from_millis(3));
            Ok(b"null".to_vec())
        })
        .unwrap();
    engine.set_execution_budget(Duration::from_millis(100));
    engine.eval(b"bridge(0)", "first.js", false).unwrap();
    let remaining = engine.execution_budget_remaining().unwrap();
    assert!(remaining < Duration::from_millis(100));
    std::thread::sleep(Duration::from_millis(15));
    assert_eq!(engine.execution_budget_remaining(), Some(remaining));
    for _ in 0..100 {
        if engine.execution_budget_exhausted() {
            break;
        }
        let _ = engine.eval(b"bridge(0)", "next.js", false);
    }
    assert!(engine.execution_budget_exhausted());
    assert!(
        engine
            .eval(b"1", "stopped.js", false)
            .unwrap_err()
            .contains("budget exhausted")
    );
    assert!(
        engine
            .execute_pending_job()
            .unwrap_err()
            .contains("budget exhausted")
    );
    assert!(
        engine
            .dispatch_json(b"{}")
            .unwrap_err()
            .contains("budget exhausted")
    );
}

#[test]
fn budget_is_shared_with_an_endless_promise_chain() {
    let mut engine = Engine::with_limits(Limits {
        time_slice: Duration::from_millis(2),
        ..Limits::default()
    })
    .unwrap();
    engine
        .eval(
            b"function again(){Promise.resolve().then(again)};again()",
            "promise.js",
            false,
        )
        .unwrap();
    engine.set_execution_budget(Duration::from_millis(5));
    for _ in 0..1_000_000 {
        if engine.execution_budget_exhausted() {
            break;
        }
        let _ = engine.execute_pending_job();
    }
    assert!(engine.execution_budget_exhausted());
    assert!(engine.has_pending_jobs());
    assert!(engine.execute_pending_job().is_err());
}

#[test]
fn native_callback_overrun_exhausts_budget_and_diagnostics_cannot_renew_it() {
    let mut engine = Engine::new().unwrap();
    engine
        .bind_host_handler("bridge", |_| {
            std::thread::sleep(Duration::from_millis(10));
            Ok(b"null".to_vec())
        })
        .unwrap();
    engine.set_execution_budget(Duration::from_millis(2));
    let error = engine
        .eval(
            b"bridge(0);throw {toString(){while(true){}}}",
            "overrun.js",
            false,
        )
        .unwrap_err();
    assert!(error.contains("budget exhausted"), "{error}");
    assert!(engine.execution_budget_exhausted());
    assert_eq!(engine.execution_budget_remaining(), Some(Duration::ZERO));
    assert!(engine.eval(b"1", "after.js", false).is_err());
}
