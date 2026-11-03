use crate::javascript::{Report, Runtime};
use crate::scripts::Scripts;
use serde_json::json;

fn page(runtime: &mut Runtime, generation: u64, html: &str) {
    let mut scripts = Scripts::default();
    scripts.extract(&rp5::parse(html), "https://fixture.test/page");
    runtime.replace_page(generation, "https://fixture.test/page", scripts.sources);
}
fn until(runtime: &mut Runtime, predicate: impl Fn(&[Report]) -> bool) -> Vec<Report> {
    let mut reports = Vec::new();
    let waiting = std::time::Instant::now();
    while waiting.elapsed() < std::time::Duration::from_secs(3) {
        std::thread::yield_now();
        reports.extend(runtime.poll());
        if predicate(&reports) {
            return reports;
        }
    }
    panic!("console work did not complete: {reports:?}");
}
fn pastes(reports: &[Report]) -> usize {
    reports
        .iter()
        .filter(|r| matches!(r, Report::Pasted { .. }))
        .count()
}
fn output(reports: &[Report]) -> Vec<&str> {
    reports
        .iter()
        .filter_map(|r| match r {
            Report::Console { batch, .. } => Some(&batch.messages),
            _ => None,
        })
        .flatten()
        .map(|m| m.text.as_str())
        .collect()
}
fn success(reports: &[Report]) {
    for report in reports {
        match report {
            Report::Pasted { result, .. }
            | Report::Evaluated { result, .. }
            | Report::Input { result, .. } => assert!(result.is_ok(), "{report:?}"),
            Report::JobError { .. } => panic!("{report:?}"),
            _ => {}
        }
    }
}

#[test]
fn paste_explicitly_starts_console_and_helpers_without_running_collected_scripts() {
    let mut runtime = Runtime::start().unwrap();
    assert!(
        runtime
            .execute_pasted("console.log('no page')".into())
            .is_err()
    );
    page(
        &mut runtime,
        1,
        "<script>throw Error('collected script ran')</script>",
    );
    for _ in 0..8 {
        assert!(output(&runtime.poll()).is_empty());
    }
    runtime.execute_pasted("globalThis.value=42; console.log('%s %d',atob(btoa('ready')),value); if(!Number.isFinite(performance.now()))throw Error('clock');".into()).unwrap();
    let reports = until(&mut runtime, |r| pastes(r) == 1);
    success(&reports);
    assert_eq!(output(&reports), ["ready 42"]);
    assert!(
        !reports
            .iter()
            .any(|r| matches!(r, Report::Evaluated { .. }))
    );
}

#[test]
fn pasted_sources_share_state_and_promise_order_with_explicit_page_run() {
    let mut runtime = Runtime::start().unwrap();
    page(
        &mut runtime,
        2,
        "<script>console.log('page', shared);</script>",
    );
    runtime.execute_pasted("globalThis.shared=1;console.log('paste');Promise.resolve().then(()=>{shared=2;console.log('job')});".into()).unwrap();
    runtime
        .execute_pasted("console.log('second',shared)".into())
        .unwrap();
    let reports = until(&mut runtime, |r| pastes(r) == 2);
    success(&reports);
    assert_eq!(output(&reports), ["paste", "job", "second 2"]);
    runtime.run().unwrap();
    let reports = until(&mut runtime, |r| {
        r.iter().any(|r| matches!(r, Report::Evaluated { .. }))
    });
    success(&reports);
    assert_eq!(output(&reports), ["page 2"]);
}

#[test]
fn logs_from_input_callbacks_and_their_jobs_reach_console() {
    let mut runtime = Runtime::start().unwrap();
    page(&mut runtime, 3, "");
    runtime.execute_pasted("hullInput.addEventListener('focus', e=>{console.info('focus', e.focused);Promise.resolve().then(()=>console.debug('job'))});".into()).unwrap();
    success(&until(&mut runtime, |r| pastes(r) == 1));
    runtime.enqueue_input(3, json!({"kind":"focus","window_id":1,"focused":true}));
    let reports = until(&mut runtime, |r| output(r).len() == 2);
    success(&reports);
    assert_eq!(output(&reports), ["focus true", "job"]);
}

#[test]
fn navigation_discards_pastes_and_context_and_releases_queue_capacity() {
    let gate = "https://fixture.test/paste-gate.js";
    crate::netfs::install([(gate.to_owned(), b"globalThis.gatePassed=true".to_vec())]);
    crate::netfs::delay(gate, 1000000);
    let mut runtime = Runtime::start().unwrap();
    runtime.enable_resource_loading();
    page(
        &mut runtime,
        4,
        "<script>globalThis.old=true;</script><script src='https://fixture.test/paste-gate.js'></script>",
    );
    runtime.run().unwrap();
    success(&until(&mut runtime, |reports| {
        reports
            .iter()
            .any(|r| matches!(r, Report::Evaluated { .. }))
    }));
    for _ in 0..8 {
        runtime
            .execute_pasted("throw Error('stale')".into())
            .unwrap();
    }
    assert!(runtime.execute_pasted("".into()).is_err());
    page(&mut runtime, 5, "");
    for _ in 0..8 {
        assert_eq!(pastes(&runtime.poll()), 0);
    }
    runtime
        .execute_pasted(
            "if(typeof old!=='undefined')throw Error('old context');console.log('new')".into(),
        )
        .unwrap();
    let reports = until(&mut runtime, |r| pastes(r) == 1);
    success(&reports);
    assert_eq!(output(&reports), ["new"]);
    assert!(
        reports
            .iter()
            .all(|r| !matches!(r, Report::Pasted { generation: 4, .. }))
    );
}

#[test]
fn native_log_budget_reports_loss_and_script_errors_do_not_discard_prior_logs() {
    let mut runtime = Runtime::start().unwrap();
    page(&mut runtime, 6, "");
    assert!(runtime.execute_pasted(" ".repeat(65537)).is_err());
    runtime
        .execute_pasted("for(let i=0;i<70;i++)console.log(i);throw Error('after logs')".into())
        .unwrap();
    let reports = until(&mut runtime, |r| pastes(r) == 1);
    assert_eq!(output(&reports).len(), 64);
    assert!(
        reports
            .iter()
            .any(|r| matches!(r, Report::Console { batch, .. } if batch.dropped==6))
    );
    assert!(
        reports
            .iter()
            .any(|r| matches!(r, Report::Pasted { result:Err(e), .. } if e.contains("after logs")))
    );
    runtime
        .execute_pasted("console.log('recovered')".into())
        .unwrap();
    let reports = until(&mut runtime, |r| pastes(r) == 1);
    success(&reports);
    assert_eq!(output(&reports), ["recovered"]);
}
