use hull_quickjs::{ConsoleLevel, Engine};
use std::time::{Duration, Instant};

fn console() -> Engine {
    let mut engine = Engine::new().unwrap();
    engine.install_console().unwrap();
    engine
}

fn eval(engine: &mut Engine, source: &str) {
    engine
        .eval(source.as_bytes(), "console-test.js", false)
        .unwrap();
}

#[test]
fn console_levels_assertion_and_empty_log_reach_owned_records() {
    let mut engine = console();
    eval(
        &mut engine,
        r#"
        console.log();
        console.debug('debug');
        console.info('info');
        console.warn('warn');
        console.error('error');
        console.assert(true, 'must not print');
        console.assert(false);
        console.assert(0, 'value=%d', 42);
    "#,
    );
    let batch = engine.take_console();
    assert_eq!(batch.dropped, 0);
    let actual: Vec<_> = batch
        .messages
        .iter()
        .map(|line| (line.level, line.text.as_str()))
        .collect();
    assert_eq!(
        actual,
        [
            (ConsoleLevel::Log, ""),
            (ConsoleLevel::Debug, "debug"),
            (ConsoleLevel::Info, "info"),
            (ConsoleLevel::Warn, "warn"),
            (ConsoleLevel::Error, "error"),
            (ConsoleLevel::Error, "Assertion failed"),
            (ConsoleLevel::Error, "Assertion failed: value=42"),
        ]
    );
    assert!(engine.take_console().messages.is_empty());
}

#[test]
fn formatting_handles_common_substitutions_and_preserves_extra_arguments() {
    let mut engine = console();
    eval(
        &mut engine,
        r#"
        console.log('%s %d %i %f %o %O %%', 'text', '4.5', '12px', '2.25px', {a:1}, [2,3]);
        console.log('%s remains %d', 'one');
        console.log('extra', null, undefined, true, 5n, Symbol('id'));
    "#,
    );
    let batch = engine.take_console();
    let actual: Vec<_> = batch
        .messages
        .iter()
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(
        actual,
        [
            "text 4.5 12 2.25 {\"a\":1} [2,3] %",
            "one remains %d",
            "extra null undefined true 5 Symbol(id)",
        ]
    );
}

#[test]
fn console_keeps_its_sink_private_and_installs_no_browser_or_shell_api() {
    let mut engine = console();
    eval(
        &mut engine,
        r#"
        for (const name of ['__hullConsoleWrite', 'document', 'window', 'fetch', 'process', 'require',
                'std', 'os', 'setTimeout', 'requestAnimationFrame']) {
            if (Object.hasOwn(globalThis, name)) throw Error('unexpected host binding: ' + name);
        }
        globalThis.__hullConsoleWrite = () => { throw Error('sink overwritten'); };
        console.log('still captured');
        delete globalThis.__hullConsoleWrite;
    "#,
    );
    assert_eq!(engine.take_console().messages[0].text, "still captured");
}

#[test]
fn console_queue_is_bounded_and_reports_drops_then_recovers_after_drain() {
    let mut engine = console();
    eval(
        &mut engine,
        "for(let index=0; index<70; index++) console.log('line', index)",
    );
    let batch = engine.take_console();
    assert_eq!(batch.messages.len(), 64);
    assert_eq!(batch.dropped, 6);
    for (index, line) in batch.messages.iter().enumerate() {
        assert_eq!(line.text, format!("line {index}"));
    }
    let drained = engine.take_console();
    assert!(drained.messages.is_empty());
    assert_eq!(drained.dropped, 0);
    eval(&mut engine, "console.log('after drain')");
    let recovered = engine.take_console();
    assert_eq!(recovered.messages.len(), 1);
    assert_eq!(recovered.messages[0].text, "after drain");
    assert_eq!(recovered.dropped, 0);
}

#[test]
fn console_truncates_at_utf8_boundaries_without_losing_embedded_nul() {
    let mut engine = console();
    eval(
        &mut engine,
        r#"
        console.log('x'.repeat(9000));
        console.log('😀'.repeat(2000));
        console.log('x'.repeat(4095) + '😀');
        console.log('before\0after');
    "#,
    );
    let batch = engine.take_console();
    assert_eq!(batch.messages.len(), 4);
    assert_eq!(batch.messages[0].text, "x".repeat(4096));
    assert_eq!(batch.messages[1].text, "😀".repeat(1024));
    assert_eq!(batch.messages[2].text, "x".repeat(4095));
    assert_eq!(batch.messages[3].text, "before\0after");
    assert!(batch.messages.iter().all(|line| line.text.len() <= 4096));
}

#[test]
fn console_falls_back_for_cyclic_objects_and_preserves_native_errors() {
    let mut engine = console();
    eval(
        &mut engine,
        "let cycle={};cycle.self=cycle;console.log(cycle);console.log({toJSON(){throw Error('json failed')}})",
    );
    let batch = engine.take_console();
    assert_eq!(batch.messages.len(), 2);
    assert_eq!(batch.messages[0].text, "[object Object]");
    assert_eq!(batch.messages[1].text, "[object Object]");
    let error = engine
        .eval(
            b"console.log('%s', {toString(){throw Error('coercion failure')}})",
            "coercion-test.js",
            false,
        )
        .unwrap_err();
    assert!(error.contains("coercion failure"), "{error}");
    assert!(engine.take_console().messages.is_empty());
    eval(&mut engine, "console.log('recovered')");
    assert_eq!(engine.take_console().messages[0].text, "recovered");
}

#[test]
fn console_coercion_is_interrupted_and_the_context_can_log_again() {
    let mut engine = console();
    let start = Instant::now();
    let error = engine
        .eval(
            b"console.log('%s', {toString(){while(true){}}})",
            "looping-coercion.js",
            false,
        )
        .unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(error.to_lowercase().contains("interrupt"), "{error}");
    eval(&mut engine, "console.log('after interruption')");
    assert_eq!(engine.take_console().messages[0].text, "after interruption");
}

#[test]
fn monotonic_clock_is_explicit_and_independent_of_javascript_wall_time() {
    let mut engine = Engine::new().unwrap();
    eval(
        &mut engine,
        "if(typeof performance !== 'undefined') throw Error('clock installed implicitly')",
    );
    engine.install_clock().unwrap();
    eval(
        &mut engine,
        r#"
        Date.now = () => -1;
        let previous = performance.now();
        if (!Number.isFinite(previous) || previous < 0) throw Error('invalid initial clock');
        for (let index=0; index<100; index++) {
            const current = performance.now();
            if (!Number.isFinite(current) || current < previous) throw Error('clock moved backward');
            previous = current;
        }
        if (Object.keys(performance).join(',') !== 'now' || typeof setTimeout !== 'undefined')
            throw Error('clock exposed scheduling capabilities');
    "#,
    );
}

#[test]
fn lone_surrogates_stay_within_the_utf8_text_budget() {
    let mut engine = Engine::new().unwrap();
    engine.install_console().unwrap();
    engine
        .eval(
            br"console.log('\ud800'.repeat(2000))",
            "surrogates.js",
            false,
        )
        .unwrap();
    let batch = engine.take_console();
    assert_eq!(batch.messages.len(), 1);
    assert!(batch.messages[0].text.len() <= 4096);
    assert!(!batch.messages[0].text.is_empty());
}
