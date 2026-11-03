use crate::{
    javascript::{Report, Runtime},
    scripts::Scripts,
};
use std::time::{Duration, Instant};
fn begin(runtime: &mut Runtime, generation: u64, html: &str) {
    let doc = rp5::parse(html);
    let mut scripts = Scripts::default();
    scripts.extract(&doc, "https://fixture.test/timers");
    runtime.begin_page(
        generation,
        "https://fixture.test/timers",
        scripts.sources,
        doc,
    );
}
fn snapshot(runtime: &mut Runtime, generation: u64) -> (rp5::Document, Vec<Report>) {
    let start = Instant::now();
    let mut reports = Vec::new();
    loop {
        reports.extend(runtime.poll());
        if let Some(doc) = runtime.take_initial_document(generation) {
            reports.extend(runtime.poll());
            return (doc, reports);
        }
        assert!(start.elapsed() < Duration::from_secs(2), "{reports:?}");
        std::thread::yield_now();
    }
}
fn output(reports: &[Report]) -> Vec<String> {
    reports
        .iter()
        .filter_map(|r| match r {
            Report::Console { batch, .. } => Some(&batch.messages),
            _ => None,
        })
        .flatten()
        .map(|m| m.text.clone())
        .collect()
}
fn success(reports: &[Report]) {
    for r in reports {
        match r {
            Report::Evaluated { result, .. } | Report::Pasted { result, .. } => {
                assert!(result.is_ok(), "{r:?}")
            }
            Report::JobError { .. } => panic!("{r:?}"),
            _ => {}
        }
    }
}

#[test]
fn zero_timers_run_as_tasks_with_arguments_cancellation_and_promise_checkpoints() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        1,
        r#"<p>before</p><script>
        console.log('script');
        const cancelled=setTimeout(()=>{throw Error('cancelled')},0);clearTimeout(cancelled);
        setTimeout(function(a,b){if(this!==globalThis||a!==7||b!=='x'||document.currentScript!==null)throw Error('callback context');
            document.querySelector('p').textContent='timer';console.log('timer');Promise.resolve().then(()=>console.log('job'));},0,7,'x');
        setTimeout(()=>console.log('second'),-4);
        Promise.resolve().then(()=>console.log('source job'));
    </script><script>console.log('next script')</script>"#,
    );
    let (doc, reports) = snapshot(&mut runtime, 1);
    success(&reports);
    assert_eq!(doc.text_content(doc.root).contains("timer"), true);
    assert_eq!(
        output(&reports),
        [
            "script",
            "source job",
            "next script",
            "timer",
            "job",
            "second"
        ]
    );
}

#[test]
fn future_timer_does_not_delay_snapshot_and_navigation_cancels_it() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        1,
        "<p>before</p><script>setTimeout(()=>console.log('old timer'),40)</script>",
    );
    let (_, reports) = snapshot(&mut runtime, 1);
    success(&reports);
    assert!(output(&reports).is_empty());
    begin(
        &mut runtime,
        2,
        "<p>new</p><script>setTimeout(()=>console.log('new timer'),0)</script>",
    );
    let (_, mut reports) = snapshot(&mut runtime, 2);
    std::thread::sleep(Duration::from_millis(50));
    for _ in 0..8 {
        reports.extend(runtime.poll());
    }
    success(&reports);
    assert_eq!(output(&reports), ["new timer"]);
}

#[test]
fn delayed_callbacks_run_after_snapshot_without_mutating_frozen_document() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        1,
        "<p>before</p><script>setTimeout(()=>{document.querySelector('p').textContent='late';console.log('late')},20)</script>",
    );
    let (doc, mut reports) = snapshot(&mut runtime, 1);
    let frozen = doc.text_content(doc.root);
    std::thread::sleep(Duration::from_millis(25));
    for _ in 0..12 {
        reports.extend(runtime.poll());
    }
    success(&reports);
    assert_eq!(output(&reports), ["late"]);
    assert_eq!(doc.text_content(doc.root), frozen);
    assert!(runtime.take_initial_document(1).is_none());
}

#[test]
fn throwing_callback_does_not_discard_other_timers_and_queue_is_bounded() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        1,
        r#"<script>
        let rejected=false;try{setTimeout('evil()',0)}catch(e){rejected=e instanceof TypeError}if(!rejected)throw Error('string callback');
        for(let i=0;i<256;i++)setTimeout(()=>{},100000);
        rejected=false;try{setTimeout(()=>{},0)}catch(e){rejected=e.name==='QuotaExceededError'}if(!rejected)throw Error('quota');
    </script>"#,
    );
    let (_, reports) = snapshot(&mut runtime, 1);
    success(&reports);
    begin(
        &mut runtime,
        2,
        "<script>setTimeout(()=>{throw Error('timer fault')},0);setTimeout(()=>console.log('survived'),0)</script>",
    );
    let (_, reports) = snapshot(&mut runtime, 2);
    assert!(
        reports
            .iter()
            .any(|r| matches!(r,Report::JobError{error,..} if error.contains("timer fault")))
    );
    assert_eq!(output(&reports), ["survived"]);
}

#[test]
fn infinite_timer_callback_still_spends_the_page_budget() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        1,
        "<p>ready</p><script>setTimeout(()=>{while(true){}},0)</script>",
    );
    let start = Instant::now();
    let (_, reports) = snapshot(&mut runtime, 1);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        reports
            .iter()
            .any(|r| matches!(r,Report::JobError{error,..} if error.contains("interrupted")))
    );
}

#[test]
fn intervals_recur_with_arguments_and_promise_checkpoints_without_ui_ticks() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        31,
        r#"<script>
        let count=0;const id=setInterval(function(value){if(this!==globalThis||value!==7)throw Error('context');
            const n=++count;console.log('tick '+n);Promise.resolve().then(()=>console.log('job '+n));if(count===3)clearInterval(id);
        },10,7);
    </script>"#,
    );
    let (_, mut reports) = snapshot(&mut runtime, 31);
    std::thread::sleep(Duration::from_millis(60));
    reports.extend(runtime.poll());
    let start = Instant::now();
    while output(&reports).len() < 6 && start.elapsed() < Duration::from_secs(1) {
        reports.extend(runtime.poll());
        std::thread::yield_now();
    }
    success(&reports);
    assert_eq!(
        output(&reports),
        ["tick 1", "job 1", "tick 2", "job 2", "tick 3", "job 3"]
    );
}

#[test]
fn either_clear_function_cancels_either_timer_kind_and_interval_faults_can_repeat() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        32,
        r#"<script>
        clearTimeout(setInterval(()=>{throw Error('cancelled interval')},0));
        clearInterval(setTimeout(()=>{throw Error('cancelled timeout')},0));
        let count=0;const id=setInterval(()=>{if(++count===1)throw Error('interval fault');clearTimeout(id);console.log('recovered');},0);
    </script>"#,
    );
    let (_, reports) = snapshot(&mut runtime, 32);
    assert_eq!(output(&reports), ["recovered"]);
    let faults: Vec<_> = reports
        .iter()
        .filter_map(|r| match r {
            Report::JobError { error, .. } => Some(error),
            _ => None,
        })
        .collect();
    assert_eq!(faults.len(), 1);
    assert!(faults[0].contains("interval fault"));
}
