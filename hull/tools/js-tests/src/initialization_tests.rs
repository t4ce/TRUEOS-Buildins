use crate::{
    javascript::{Report, Runtime},
    scripts::Scripts,
};
use std::time::{Duration, Instant};

fn begin(runtime: &mut Runtime, generation: u64, html: &str) {
    let document = rp5::parse(html);
    let mut scripts = Scripts::default();
    scripts.extract(&document, "https://fixture.test/page");
    runtime.begin_page(
        generation,
        "https://fixture.test/page",
        scripts.sources,
        document,
    );
}
fn snapshot(runtime: &mut Runtime, generation: u64) -> (rp5::Document, Vec<Report>) {
    let started = Instant::now();
    let mut reports = Vec::new();
    loop {
        reports.extend({
            std::thread::sleep(Duration::from_millis(1));
            runtime.poll()
        });
        if let Some(document) = runtime.take_initial_document(generation) {
            reports.extend(runtime.poll());
            return (document, reports);
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "initialization did not release layout: {reports:?}"
        );
        std::thread::yield_now();
    }
}
fn body_text(document: &rp5::Document) -> String {
    let body = document
        .nodes
        .iter()
        .enumerate()
        .find(|(id, _)| document.name(*id) == "body")
        .unwrap()
        .0;
    document.text_content(body)
}

#[test]
fn dom_ready_runs_after_sources_and_their_jobs_before_render_capture() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        30,
        r#"<p>before</p><script>
        let trace=[];const p=document.querySelector('p');
        if(document.readyState!=='loading')throw Error('initial ready state');
        document.addEventListener('readystatechange',()=>trace.push(document.readyState));
        document.addEventListener('DOMContentLoaded',function(e){
            if(this!==document||e.target!==document||!e.bubbles||!e.isTrusted||document.currentScript!==null)throw Error('ready semantics');
            if(trace.join(',')!=='first,promise,second,interactive')throw Error(trace);
            if(hullInput.snapshot().received!=='0')throw Error('lifecycle counted as raw input');
            p.textContent='ready';Promise.resolve().then(()=>p.textContent+=' job');
        },{once:true});
        trace.push('first');Promise.resolve().then(()=>trace.push('promise'));
    </script><script>trace.push('second')</script>"#,
    );
    let (document, reports) = snapshot(&mut runtime, 30);
    let p = document
        .nodes
        .iter()
        .enumerate()
        .find(|(id, _)| document.name(*id) == "p")
        .unwrap()
        .0;
    assert_eq!(document.text_content(p), "ready job");
    assert!(
        !reports.iter().any(|r| matches!(
            r,
            Report::Evaluated { result: Err(_), .. } | Report::JobError { .. }
        )),
        "{reports:?}"
    );
    runtime
        .execute_pasted(
            "if(document.readyState!=='interactive')throw Error('ready state lost')".into(),
        )
        .unwrap();
    for _ in 0..16 {
        for report in {
            std::thread::sleep(Duration::from_millis(1));
            runtime.poll()
        } {
            if let Report::Pasted { result, .. } = report {
                result.unwrap();
                return;
            }
        }
    }
    panic!("paste not executed");
}

#[test]
fn current_script_tracks_classic_identity_and_parent_but_not_jobs_modules_or_pastes() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        20,
        r#"<div id="container"><p id="result">before</p>
<script id="first">
    const self=document.currentScript;
    if(self!==document.getElementById('first')||self.parentElement!==document.getElementById('container'))throw Error('classic identity/parent');
    function readCurrent(){return document.currentScript&&document.currentScript.getAttribute('id')}
    Promise.resolve().then(()=>{if(document.currentScript!==null)throw Error('microtask retained currentScript');document.querySelector('p').setAttribute('data-job','ok')});
</script>
<script id="second">
    if(readCurrent()!=='second')throw Error('definition script leaked into synchronous call');
    Promise.resolve().then(()=>{if(document.currentScript!==null)throw Error('error-path job retained currentScript')});
    throw Error('intentional classic failure');
</script>
<script id="bad">const invalid = ;</script>
<script type="module">if(document.currentScript!==null)throw Error('module currentScript');document.querySelector('p').setAttribute('data-module','ok')</script>
<script id="last">if(readCurrent()!=='last')throw Error('next classic scope');document.querySelector('p').textContent='ready'</script>
</div>"#,
    );
    let (document, reports) = snapshot(&mut runtime, 20);
    let p = document
        .nodes
        .iter()
        .enumerate()
        .find(|(id, _)| document.name(*id) == "p")
        .unwrap()
        .0;
    assert_eq!(document.text_content(p), "ready");
    let attrs = document.element(p).unwrap().2;
    assert!(
        attrs
            .iter()
            .any(|a| a.name == "data-job" && a.value == "ok")
    );
    assert!(
        attrs
            .iter()
            .any(|a| a.name == "data-module" && a.value == "ok")
    );
    let errors = reports
        .iter()
        .filter_map(|r| {
            if let Report::Evaluated { result: Err(e), .. } = r {
                Some(e.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors
            .iter()
            .any(|e| e.contains("intentional classic failure"))
    );
    assert!(errors.iter().any(|e| e.contains("SyntaxError")));
    assert!(
        !reports.iter().any(|r| matches!(r, Report::JobError { .. })),
        "{reports:?}"
    );
    runtime
        .execute_pasted(
            "if(document.currentScript!==null)throw Error('paste retained script')".into(),
        )
        .unwrap();
    let mut pasted = false;
    for _ in 0..32 {
        for report in {
            std::thread::sleep(Duration::from_millis(1));
            runtime.poll()
        } {
            if let Report::Pasted { result, .. } = report {
                result.unwrap();
                pasted = true;
            }
        }
        if pasted {
            break;
        }
    }
    assert!(pasted);
}

#[test]
fn current_script_keeps_its_handle_when_its_parent_content_is_replaced() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        21,
        r#"<div id="container"><script id="self">
        const me=document.currentScript;
        me.parentElement.textContent='replaced';
        if(document.currentScript!==me||me.parentElement!==null)throw Error('detachment lost active script');
        Promise.resolve().then(()=>{if(document.currentScript!==null)throw Error('detached script leaked into job')});
    </script></div>"#,
    );
    let (document, reports) = snapshot(&mut runtime, 21);
    assert_eq!(body_text(&document), "replaced");
    assert!(
        !reports.iter().any(|r| matches!(
            r,
            Report::Evaluated { result: Err(_), .. } | Report::JobError { .. }
        )),
        "{reports:?}"
    );
}

#[test]
fn initial_scripts_and_promise_jobs_mutate_the_real_document_before_capture() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        1,
        "<p id='message'>before</p><script>document.getElementById('message').textContent='script';Promise.resolve().then(()=>document.querySelector('p').textContent='promise')</script><script>const p=document.querySelector('p');if(p.textContent!=='promise')throw Error('order');p.setAttribute('style','color:red;background-color:black');</script>",
    );
    let (document, reports) = snapshot(&mut runtime, 1);
    let p = document
        .nodes
        .iter()
        .enumerate()
        .find(|(id, _)| document.name(*id) == "p")
        .unwrap()
        .0;
    assert_eq!(document.text_content(p), "promise");
    assert!(
        document
            .element(p)
            .unwrap()
            .2
            .iter()
            .any(|a| a.name == "style" && a.value == "color:red;background-color:black")
    );
    assert!(reports.iter().any(|r| matches!(
        r,
        Report::Initialized {
            capped: false,
            remaining_sources: 0,
            ..
        }
    )));
    assert!(!reports.iter().any(|r| matches!(
        r,
        Report::Evaluated { result: Err(_), .. } | Report::JobError { .. }
    )));
}

#[test]
fn frozen_document_does_not_follow_later_live_dom_changes() {
    let mut runtime = Runtime::start().unwrap();
    begin(&mut runtime, 2, "<p id='message'>first frame</p>");
    let (document, _) = snapshot(&mut runtime, 2);
    runtime.execute_pasted("const live=document.querySelector('p');live.textContent='later';live.setAttribute('style','color:red');live.setAttribute('data-late','yes');console.log(live.textContent)".into()).unwrap();
    let mut output = Vec::new();
    for _ in 0..32 {
        for report in {
            std::thread::sleep(Duration::from_millis(1));
            runtime.poll()
        } {
            if let Report::Console { batch, .. } = report {
                output.extend(batch.messages.into_iter().map(|m| m.text));
            }
        }
        if !output.is_empty() {
            break;
        }
    }
    assert_eq!(output, ["later"]);
    assert_eq!(body_text(&document), "first frame");
    let p = document
        .nodes
        .iter()
        .enumerate()
        .find(|(id, _)| document.name(*id) == "p")
        .unwrap()
        .0;
    assert!(
        !document
            .element(p)
            .unwrap()
            .2
            .iter()
            .any(|a| matches!(a.name.as_str(), "style" | "data-late"))
    );
    assert!(
        runtime.take_initial_document(2).is_none(),
        "mutation produced a second render snapshot"
    );
}

#[test]
fn finite_script_beyond_old_call_slice_can_contribute_to_initial_snapshot() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        31,
        "<p>before</p><script>const until=performance.now()+45;while(performance.now()<until){}document.querySelector('p').textContent='admitted'</script>",
    );
    let (document, reports) = snapshot(&mut runtime, 31);
    let p = document
        .nodes
        .iter()
        .enumerate()
        .find(|(id, _)| document.name(*id) == "p")
        .unwrap()
        .0;
    assert_eq!(document.text_content(p), "admitted");
    assert!(
        !reports.iter().any(|r| matches!(
            r,
            Report::Evaluated { result: Err(_), .. } | Report::JobError { .. }
        )),
        "{reports:?}"
    );
}

#[test]
fn missing_host_api_fault_does_not_prevent_later_sources_or_layout() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        3,
        "<p>before</p><script>fetch('/data')</script><script>document.querySelector('p').textContent='after error';for(const name of ['window','fetch','XMLHttpRequest','require','process','std','os'])if(typeof globalThis[name]!=='undefined')throw Error('host API leak')</script>",
    );
    let (document, reports) = snapshot(&mut runtime, 3);
    let p = document
        .nodes
        .iter()
        .enumerate()
        .find(|(id, _)| document.name(*id) == "p")
        .unwrap()
        .0;
    assert_eq!(document.text_content(p), "after error");
    assert!(
        reports
            .iter()
            .any(|r| matches!(r, Report::Evaluated { result:Err(e), .. } if e.contains("fetch")))
    );
    assert!(
        reports
            .iter()
            .any(|r| matches!(r, Report::Evaluated { result: Ok(()), .. }))
    );
}

#[test]
fn navigation_discards_pending_initialization_and_old_document_handles() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        4,
        "<script>globalThis.old=document;</script><script>throw Error('stale source ran')</script>",
    );
    begin(
        &mut runtime,
        5,
        "<p>new</p><script>if(typeof old!=='undefined')throw Error('old globals');document.querySelector('p').textContent='new page'</script>",
    );
    assert!(runtime.take_initial_document(4).is_none());
    let (document, reports) = snapshot(&mut runtime, 5);
    let p = document
        .nodes
        .iter()
        .enumerate()
        .find(|(id, _)| document.name(*id) == "p")
        .unwrap()
        .0;
    assert_eq!(document.text_content(p), "new page");
    assert!(
        !reports
            .iter()
            .any(|r| matches!(r, Report::Evaluated { result: Err(_), .. }))
    );
}

#[test]
fn one_total_execution_budget_stops_many_infinite_sources_and_keeps_completed_mutations() {
    let mut runtime = Runtime::start().unwrap();
    let scripts =
        "<script>document.querySelector('p').textContent='committed';while(true){}</script>"
            .repeat(20);
    let started = Instant::now();
    begin(&mut runtime, 6, &format!("<p>before</p>{scripts}"));
    let (document, reports) = snapshot(&mut runtime, 6);
    assert!(started.elapsed() < Duration::from_secs(2));
    let p = document
        .nodes
        .iter()
        .enumerate()
        .find(|(id, _)| document.name(*id) == "p")
        .unwrap()
        .0;
    assert_eq!(document.text_content(p), "committed");
    assert!(reports.iter().any(|r| matches!(r, Report::Initialized { capped:true, remaining_sources, .. } if *remaining_sources>0)));
    assert!(
        reports
            .iter()
            .filter(|r| matches!(r, Report::Evaluated { .. }))
            .count()
            < 20
    );
    // If the wall cap precedes the last fraction of active budget, one more
    // bounded invocation spends it; a new source never resets the allowance.
    if runtime.execute_pasted("while(true){}".into()).is_ok() {
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            if runtime
                .poll()
                .iter()
                .any(|report| matches!(report, Report::BudgetExhausted { .. }))
            {
                break;
            }
            assert!(
                Instant::now() < until,
                "execution allowance did not exhaust"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert!(
        runtime
            .execute_pasted("console.log('budget reset')".into())
            .is_err()
    );
}

#[test]
fn endless_microtasks_cannot_hold_the_initial_render_snapshot() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        7,
        "<p>ready</p><script>function again(){Promise.resolve().then(again)}again()</script><script>document.querySelector('p').textContent='must not run after cutoff'</script>",
    );
    let started = Instant::now();
    let mut reports = Vec::new();
    let document = loop {
        reports.extend({
            std::thread::sleep(Duration::from_millis(1));
            runtime.poll()
        });
        if let Some(document) = runtime.take_initial_document(7) {
            reports.extend(runtime.poll());
            break document;
        }
        assert!(started.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(20));
    };
    let p = document
        .nodes
        .iter()
        .enumerate()
        .find(|(id, _)| document.name(*id) == "p")
        .unwrap()
        .0;
    assert_eq!(document.text_content(p), "ready");
    assert!(
        reports
            .iter()
            .any(|r| matches!(r, Report::Initialized { capped: true, .. }))
    );
}

#[test]
fn ide_html_template_logs_once_per_full_page_load() {
    let mut runtime = Runtime::start().unwrap();
    let html = format!(
        "<!doctype html><html><head><script>{}</script></head><body></body></html>",
        trueos_ide::html_preview::JS
    );
    for generation in [901, 902] {
        begin(&mut runtime, generation, &html);
        let (_, mut reports) = snapshot(&mut runtime, generation);
        for _ in 0..16 {
            reports.extend({
                std::thread::sleep(Duration::from_millis(1));
                runtime.poll()
            });
        }
        let logs: Vec<_> = reports
            .iter()
            .filter_map(|r| match r {
                Report::Console { batch, .. } => Some(&batch.messages),
                _ => None,
            })
            .flatten()
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(logs, ["template console log"]);
        assert!(
            !reports.iter().any(|r| matches!(
                r,
                Report::Evaluated { result: Err(_), .. } | Report::JobError { .. }
            )),
            "{reports:?}"
        );
    }
}

#[test]
fn cookie_snapshot_resets_on_every_navigation_and_includes_ready_jobs() {
    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        91,
        r#"<script>
        document.cookie='before=one';
        localStorage.setItem('before','local');
        document.addEventListener('DOMContentLoaded',()=>Promise.resolve().then(()=>document.cookie='ready=two'));
    </script>"#,
    );
    snapshot(&mut runtime, 91);
    assert_eq!(runtime.cookie_snapshot(), "before=one; ready=two");
    assert!(runtime.storage_snapshot().contains("localStorage:before"));
    runtime
        .execute_pasted("document.cookie='late=three'".into())
        .unwrap();
    for _ in 0..16 {
        {
            std::thread::sleep(Duration::from_millis(1));
            runtime.poll()
        };
    }
    assert_eq!(
        runtime.cookie_snapshot(),
        "before=one; ready=two; late=three"
    );
    begin(
        &mut runtime,
        92,
        "<script>if(document.cookie!==''||localStorage.length!==0)throw Error('old jar');document.cookie='fresh=yes'</script>",
    );
    let (_, reports) = snapshot(&mut runtime, 92);
    assert!(
        !reports
            .iter()
            .any(|r| matches!(r, Report::Evaluated { result: Err(_), .. }))
    );
    assert_eq!(runtime.cookie_snapshot(), "fresh=yes");
}
