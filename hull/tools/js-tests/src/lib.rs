//! Host fixtures execute Hull's production JavaScript actor and script extraction.
#![allow(dead_code)]

#[path = "../../../src/resource_url.rs"]
mod resource_url;
#[path = "../../../src/ide_preview.rs"]
mod ide_preview;

extern crate self as trueos;
extern crate self as winit;

pub mod platform {
    pub use winit_trueos as trueos;
}
pub mod event {
    pub use winit_core::event::*;
}
pub mod window {
    pub use winit_core::window::Window;
}

pub use tokio;
extern crate alloc;
#[path = "../../../src/platform.rs"]
mod platform;
#[allow(unexpected_cfgs)]
#[path = "../../../../../TRUEOS-Blueprints/api/src/worker.rs"]
pub mod native_worker;
pub mod worker {
    pub use crate::native_worker::{JoinHandle, cancellation_requested};
    pub fn spawn<F, R>(f: F) -> Result<JoinHandle<R>, crate::native_worker::SpawnError>
    where F: FnOnce() -> R + Send + 'static, R: Send + 'static {
        let inherit = crate::netfs::inherit();
        crate::native_worker::spawn(move || { inherit(); f() })
    }
}

#[path = "resource_netfs.rs"]
pub mod netfs;
pub mod runtime {
    pub use tokio::runtime::{Builder, Handle, Runtime};
    pub fn current_thread() -> Builder {
        let mut builder = Builder::new_current_thread();
        builder.enable_time();
        builder
    }
}
pub mod task {
    pub use tokio::spawn;
    pub use tokio::task::{JoinError, JoinHandle, JoinSet, LocalSet, yield_now};
}
pub mod sync {
    pub use tokio::sync::{mpsc, oneshot, watch};
}
pub mod time {
    pub use tokio::time::{Duration, Instant, sleep, timeout};
}
pub mod logl {
    pub mod level {
        pub const INFO: u32 = 1;
        pub const ERROR: u32 = 2;
    }
    pub fn log(_: u32, _: std::fmt::Arguments<'_>) {}
}

#[cfg(test)]
mod console_tests;
#[cfg(test)]
mod dom_tests;
#[cfg(test)]
mod dom_mutation_tests;
#[cfg(test)]
mod navigation_tests;
#[cfg(test)]
mod timer_tests;
#[cfg(test)]
mod video_tests;
#[cfg(test)]
mod style_tests;
#[cfg(test)]
#[path="../../../src/video.rs"]
mod video;
#[cfg(test)]
mod dom_tag_collection_tests;
#[cfg(test)]
mod jquery_event_bridge_tests;
#[cfg(test)]
mod dom_events_tests;
#[cfg(test)]
mod url_tests;
#[cfg(test)]
mod helpers_tests;
#[cfg(test)]
mod initialization_tests;
#[cfg(test)]
mod input_actor_tests;
#[cfg(test)]
mod input_adapter_tests;
#[path = "../../../src/javascript.rs"]
mod javascript;
#[path = "../../../src/javascript_dom.rs"]
mod javascript_dom;
#[path = "../../../src/javascript_input.rs"]
mod javascript_input;
#[cfg(test)]
mod jquery_tests;
#[cfg(test)]
mod resource_failure_tests;
#[cfg(test)]
mod resource_tests;
#[path = "../../../src/scripts.rs"]
mod scripts;
#[path = "../../../src/selectors.rs"]
mod selectors;
#[cfg(test)]
#[path = "../../../src/viewport.rs"]
mod viewport;
#[cfg(test)]
mod window_tests;

#[cfg(test)]
mod tests {
    use super::{
        javascript::{Report, Runtime},
        scripts::{ScriptSource, Scripts},
    };
    use std::time::{Duration, Instant};

    fn sources(html: &str) -> Vec<ScriptSource> {
        let document = rp5::parse(html);
        let mut scripts = Scripts::default();
        scripts.extract(&document, "https://fixture.test/page");
        scripts.sources
    }
    fn turns(runtime: &mut Runtime, count: usize) -> Vec<Report> {
        (0..count).flat_map(|_| {std::thread::sleep(std::time::Duration::from_millis(1));runtime.poll()}).collect()
    }
    fn until(runtime: &mut Runtime, predicate: impl Fn(&[Report]) -> bool) -> Vec<Report> {
        let mut reports = Vec::new();
        let waiting=std::time::Instant::now();
        while waiting.elapsed()<std::time::Duration::from_secs(3) {
            std::thread::yield_now();
            let turn = runtime.poll();
            // One report drain can cover multiple native worker steps.
            reports.extend(turn);
            if predicate(&reports) {
                return reports;
            }
        }
        panic!("actor did not complete bounded fixture turns");
    }
    fn completed(reports: &[Report]) -> usize {
        reports
            .iter()
            .filter(|report| matches!(report, Report::Evaluated { .. } | Report::Skipped { .. }))
            .count()
    }
    fn assert_success(reports: &[Report], count: usize) {
        let evaluations: Vec<_> = reports
            .iter()
            .filter_map(|report| match report {
                Report::Evaluated { result, .. } => Some(result),
                _ => None,
            })
            .collect();
        assert_eq!(evaluations.len(), count);
        for result in evaluations {
            assert!(result.is_ok(), "native evaluation failed: {result:?}");
        }
        assert!(
            !reports
                .iter()
                .any(|report| matches!(report, Report::JobError { .. })),
            "unexpected promise-job error"
        );
    }

    #[test]
    fn actor_starts_without_any_page_and_remains_idle() {
        let mut runtime = Runtime::start().expect("start idle actor");
        let reports = until(&mut runtime, |reports| {
            reports
                .iter()
                .any(|report| matches!(report, Report::Online))
        });
        assert_eq!(completed(&reports), 0);
        assert_eq!(completed(&turns(&mut runtime, 16)), 0);
    }

    #[test]
    fn collecting_a_page_never_automatically_evaluates_or_classifies_it() {
        let mut runtime = Runtime::start().unwrap();
        runtime.replace_page(1,"https://fixture.test/",sources("<script>throw Error('must remain inactive')</script><script type='application/json'>{broken data</script>"));
        let reports = turns(&mut runtime, 32);
        assert!(reports.iter().any(|report| matches!(
            report,
            Report::Page {
                generation: 1,
                sources: 2
            }
        )));
        assert_eq!(
            completed(&reports),
            0,
            "page replacement triggered script execution"
        );
    }

    #[test]
    fn explicit_run_preserves_script_order_and_promise_checkpoint() {
        let mut runtime = Runtime::start().unwrap();
        runtime.replace_page(11,"https://fixture.test/",sources("<script>globalThis.order=['script'];Promise.resolve().then(()=>order.push('promise'));</script><script>if(order.join(',')!=='script,promise')throw Error('script overtook promise');order.push('second');</script><script>if(order.join(',')!=='script,promise,second')throw Error('lost page state');</script>"));
        assert_eq!(completed(&turns(&mut runtime, 8)), 0);
        runtime.run().expect("explicit run command");
        let reports = until(&mut runtime, |reports| completed(reports) == 3);
        assert_success(&reports, 3);
        assert!(
            reports
                .iter()
                .filter_map(|report| match report {
                    Report::Evaluated { generation, .. } => Some(*generation),
                    _ => None,
                })
                .all(|generation| generation == 11)
        );
    }

    #[test]
    fn run_requires_a_page_and_each_page_can_be_requested_only_once() {
        let mut runtime = Runtime::start().unwrap();
        assert!(runtime.run().is_err(), "run accepted without a page");
        runtime.replace_page(12,"https://fixture.test/",sources("<script>globalThis.executions=(globalThis.executions||0)+1;if(executions!==1)throw Error('duplicate run');</script>"));
        runtime.run().unwrap();
        assert!(runtime.run().is_err(), "duplicate pending run accepted");
        assert_success(&until(&mut runtime, |reports| completed(reports) == 1), 1);
        assert!(
            runtime.run().is_err(),
            "completed page was implicitly rerun"
        );
        assert_eq!(completed(&turns(&mut runtime, 8)), 0);
    }

    #[test]
    fn long_promise_chain_finishes_before_following_script_on_native_worker() {
        let mut runtime=Runtime::start().unwrap();
        runtime.replace_page(13,"https://fixture.test/",sources("<script>globalThis.jobs=0;let chain=Promise.resolve();for(let i=0;i<40;i++)chain=chain.then(()=>jobs++);</script><script>if(jobs!==40)throw Error('script overtook pending jobs');</script>"));
        runtime.run().unwrap();assert_success(&until(&mut runtime,|reports|completed(reports)>=2),2);
    }

    #[test]
    fn navigation_discards_old_context_and_queued_scripts_then_stays_inactive() {
        let mut runtime = Runtime::start().unwrap();
        runtime.replace_page(21,"https://fixture.test/old",sources("<script>globalThis.oldPage=true;setTimeout(()=>{throw Error('stale queued timer ran')},1000);</script>"));
        runtime.run().unwrap();
        let first = until(&mut runtime, |reports| {
            reports
                .iter()
                .any(|report| matches!(report, Report::Evaluated { generation: 21, .. }))
        });
        assert_success(&first, 1);
        runtime.replace_page(22,"https://fixture.test/new",sources("<script>if(typeof oldPage!=='undefined')throw Error('old context retained');globalThis.newPage=true;</script><script>if(newPage!==true)throw Error('new page state lost');</script>"));
        let inactive = turns(&mut runtime, 16);
        assert_eq!(
            completed(&inactive),
            0,
            "navigation evaluated queued old/new work"
        );
        assert!(
            inactive
                .iter()
                .any(|report| matches!(report, Report::Page { generation: 22, .. }))
        );
        runtime.run().unwrap();
        let fresh = until(&mut runtime, |reports| completed(reports) == 2);
        assert_success(&fresh, 2);
        assert!(
            !fresh
                .iter()
                .any(|report| matches!(report, Report::Evaluated { generation: 21, .. }))
        );
    }

    #[test]
    fn native_parser_handles_javascript_and_metadata_skips_data() {
        let mut runtime = Runtime::start().unwrap();
        runtime.replace_page(31,"https://fixture.test/",sources("<script>function (</script><script type='application/json'>{not javascript</script><script type='application/ld+json'>{also data</script><script>globalThis.afterSyntaxError=true;</script>"));
        runtime.run().unwrap();
        let reports = until(&mut runtime, |reports| completed(reports) == 4);
        let results: Vec<_> = reports
            .iter()
            .filter_map(|report| match report {
                Report::Evaluated { result, .. } => Some(result),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2);
        assert!(results[0].as_ref().unwrap_err().contains("SyntaxError"));
        assert!(results[1].is_ok(), "syntax error poisoned next script");
        assert_eq!(
            reports
                .iter()
                .filter(|report| matches!(report, Report::Skipped { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn external_sources_remain_metadata_without_network_loading() {
        let page = sources(
            "<base href='https://cdn.fixture.test/base/'><script src='./outside.js#first'></script><script src='./outside.js#second'></script><script>if(typeof fetchedExternal!=='undefined')throw Error('external script executed');</script>",
        );
        assert_eq!(page.len(), 3);
        assert_eq!(page[0].origin, "https://cdn.fixture.test/base/outside.js");
        assert!(page[0].bytes.is_none());
        assert_eq!(page[0].nodes.len(), 1);
        let mut runtime = Runtime::start().unwrap();
        runtime.replace_page(41, "https://fixture.test/", page);
        runtime.run().unwrap();
        let reports = until(&mut runtime, |reports| completed(reports) == 3);
        assert_success(&reports, 1);
        assert_eq!(
            reports
                .iter()
                .filter(|report| matches!(report, Report::Skipped { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn bare_engine_exposes_no_browser_or_filesystem_functions() {
        let mut runtime = Runtime::start().unwrap();
        runtime.replace_page(51,"https://fixture.test/",sources("<script>for(const name of ['window','document','fetch','XMLHttpRequest','require','process','std','os'])if(typeof globalThis[name]!=='undefined')throw Error('unexpected host API: '+name);</script>"));
        runtime.run().unwrap();
        assert_success(&until(&mut runtime, |reports| completed(reports) == 1), 1);
    }

    #[test]
    fn infinite_script_is_interrupted_and_next_script_can_run() {
        let mut runtime = Runtime::start().unwrap();
        runtime.replace_page(
            61,
            "https://fixture.test/",
            sources(
                "<script>while(true){}</script><script>globalThis.afterInterrupt=true;</script>",
            ),
        );
        runtime.run().unwrap();
        let start = Instant::now();
        let reports = until(&mut runtime, |reports| completed(reports) == 2);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "guest evaluation froze the UI poll loop"
        );
        let results: Vec<_> = reports
            .iter()
            .filter_map(|report| match report {
                Report::Evaluated { result, .. } => Some(result),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2);
        assert!(
            results[0].is_err(),
            "infinite script did not hit engine budget"
        );
        assert!(results[1].is_ok(), "interrupt poisoned next script");
    }
}
