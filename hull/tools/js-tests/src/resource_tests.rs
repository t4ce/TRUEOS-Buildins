use crate::{
    javascript::{Report, Runtime},
    netfs,
    scripts::Scripts,
};
use std::time::{Duration, Instant};
const PAGE: &str = "https://fixture.test/path/page";
fn begin(runtime: &mut Runtime, generation: u64, html: &str) {
    let doc = rp5::parse(html);
    let mut scripts = Scripts::default();
    scripts.extract(&doc, PAGE);
    runtime.begin_page(generation, PAGE, scripts.sources, doc);
}
pub(super) fn runtime(html: &str, resources: Vec<(&str, &str)>) -> Runtime {
    netfs::install(
        resources
            .into_iter()
            .map(|(url, code)| (url.into(), code.as_bytes().to_vec())),
    );
    let mut runtime = Runtime::start().unwrap();
    runtime.enable_resource_loading();
    begin(&mut runtime, 1, html);
    runtime
}
pub(super) fn finish(runtime: &mut Runtime, generation: u64) -> (rp5::Document, Vec<Report>) {
    let start = Instant::now();
    let mut reports = Vec::new();
    loop {
        reports.extend(runtime.poll());
        if let Some(doc) = runtime.take_initial_document(generation) {
            reports.extend(runtime.poll());
            return (doc, reports);
        }
        assert!(
            start.elapsed() < Duration::from_secs(7),
            "stalled: {reports:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
pub(super) fn text(doc: &rp5::Document) -> String {
    let node = doc
        .nodes
        .iter()
        .enumerate()
        .find(|(id, _)| doc.name(*id) == "p")
        .unwrap()
        .0;
    doc.text_content(node)
}
#[test]
fn repeated_classic_elements_execute_twice_but_repeated_module_urls_execute_once() {
    let mut r = runtime(
        r#"<p>before</p><script src='./repeat.js' id='first'></script><script src='./repeat.js' id='second'></script><script type=module src='./once.js'></script><script type=module src='./once.js'></script><script>document.querySelector('p').textContent=visits.join(',')+':'+moduleRuns</script>"#,
        vec![
            (
                "https://fixture.test/path/repeat.js",
                "globalThis.visits=globalThis.visits||[];visits.push(document.currentScript.getAttribute('id'));",
            ),
            (
                "https://fixture.test/path/once.js",
                "globalThis.moduleRuns=(globalThis.moduleRuns||0)+1;",
            ),
        ],
    );
    let (doc, reports) = finish(&mut r, 1);
    assert_eq!(text(&doc), "first,second:1");
    no_errors(&reports);
    assert_eq!(netfs::requested().len(), 2);
}
#[test]
fn import_map_relative_keys_are_document_relative_and_prefixes_cannot_backtrack() {
    use crate::javascript::module_loader::Policy;
    let doc = rp5::parse(
        r#"<script type=importmap>{"imports":{"./x.js":"./mapped.js","lib/":"./library/"}}</script>"#,
    );
    let mut p = Policy::default();
    p.document_maps(&doc, PAGE).unwrap();
    assert_eq!(
        p.resolve("https://fixture.test/path/entry.js", "./x.js")
            .unwrap(),
        "https://fixture.test/path/mapped.js"
    );
    assert_eq!(
        p.resolve("https://fixture.test/other/entry.js", "./x.js")
            .unwrap(),
        "https://fixture.test/other/x.js"
    );
    assert!(p.resolve(PAGE, "lib/../escape.js").is_err());
    assert_eq!(
        p.candidates(PAGE, "https://esm.sh/gh/owner/repo")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        p.candidates(PAGE, "https://esm.sh/v135/react@18")
            .unwrap()
            .len(),
        1
    );
}
fn no_errors(reports: &[Report]) {
    assert!(
        !reports.iter().any(|r| matches!(
            r,
            Report::Evaluated { result: Err(_), .. }
                | Report::JobError { .. }
                | Report::BudgetExhausted { .. }
        )),
        "{reports:?}"
    );
}
#[test]
fn wide_static_graph_with_ui_turn_delays_waits_for_preparation_without_spending_js_time() {
    let sources = (0..40)
        .map(|i| {
            (
                format!("https://fixture.test/path/part{i}.js"),
                format!("export const value{i}={i};"),
            )
        })
        .collect::<Vec<_>>();
    let imports = (0..40)
        .map(|i| format!("import './part{i}.js';"))
        .collect::<String>();
    let html = format!(
        "<p>before</p><script type=module>{imports}document.querySelector('p').textContent='all prepared'</script>"
    );
    let mut r = runtime(
        &html,
        sources
            .iter()
            .map(|(url, code)| (url.as_str(), code.as_str()))
            .collect(),
    );
    let start = Instant::now();
    let mut reports = Vec::new();
    let doc = loop {
        reports.extend(r.poll());
        if let Some(doc) = r.take_initial_document(1) {
            break doc;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(text(&doc), "all prepared");
    assert_eq!(netfs::requested().len(), 40);
    no_errors(&reports);
}
#[test]
fn inline_module_graph_cycles_and_top_level_await_complete_before_snapshot() {
    let mut r = runtime(
        r#"<p>before</p><script type=module>import {answer} from './a.js';document.querySelector('p').textContent=String(answer);</script>"#,
        vec![
            (
                "https://fixture.test/path/a.js",
                "import {read} from './b.js';export let x=1;export function inc(){x++}await Promise.resolve();export const answer=read();",
            ),
            (
                "https://fixture.test/path/b.js",
                "import{x,inc}from './a.js';export function read(){inc();return x;}",
            ),
        ],
    );
    let (doc, reports) = finish(&mut r, 1);
    assert_eq!(text(&doc), "2");
    no_errors(&reports);
    assert_eq!(netfs::requested().len(), 2);
    assert_eq!(netfs::active(), 0);
}
#[test]
fn computed_dynamic_import_and_nested_relative_import_do_not_reexecute_callers() {
    let mut r = runtime(
        r#"<p>before</p><script>let runs=0;runs++;const path='./lazy.js';import(path).then(async m=>{const n=await m.next();if(runs!==1)throw Error('replayed caller');document.querySelector('p').textContent=String(n.answer)});</script>"#,
        vec![
            (
                "https://fixture.test/path/lazy.js",
                "export async function next(){return import('./next.js')}",
            ),
            (
                "https://fixture.test/path/next.js",
                "export const answer=42;",
            ),
        ],
    );
    let (doc, reports) = finish(&mut r, 1);
    assert_eq!(text(&doc), "42");
    no_errors(&reports);
    assert_eq!(netfs::requested().len(), 2);
}
#[test]
fn esm_primary_failure_uses_jsdelivr_and_its_own_relative_base() {
    let mut r = runtime(
        r#"<p>before</p><script>import('tiny-demo@1.0.0').then(m=>document.querySelector('p').textContent=m.answer)</script>"#,
        vec![
            (
                "https://cdn.jsdelivr.net/npm/tiny-demo@1.0.0/+esm",
                "export {answer} from './answer.js';",
            ),
            (
                "https://cdn.jsdelivr.net/npm/tiny-demo@1.0.0/answer.js",
                "export const answer='fallback';",
            ),
        ],
    );
    let (doc, reports) = finish(&mut r, 1);
    assert_eq!(text(&doc), "fallback");
    no_errors(&reports);
    let urls = netfs::requested();
    assert_eq!(
        urls[0],
        "https://esm.sh/tiny-demo@1.0.0?target=es2022&bundle"
    );
    assert!(urls.contains(&"https://cdn.jsdelivr.net/npm/tiny-demo@1.0.0/answer.js".into()));
    assert!(reports.iter().any(|r| matches!(
        r,
        Report::Resource {
            status: "fallback",
            ..
        }
    )));
}
#[test]
fn broken_primary_dependency_retries_the_package_on_fallback_before_execution() {
    let mut r = runtime(
        r#"<p>before</p><script>globalThis.runs=0;import('broken@1.0.0').then(m=>document.querySelector('p').textContent=m.answer+':'+runs)</script>"#,
        vec![
            (
                "https://esm.sh/broken@1.0.0?target=es2022&bundle",
                "import './missing.js';globalThis.runs++;export const answer='wrong';",
            ),
            (
                "https://cdn.jsdelivr.net/npm/broken@1.0.0/+esm",
                "globalThis.runs++;export const answer='fallback';",
            ),
        ],
    );
    let (doc, reports) = finish(&mut r, 1);
    assert_eq!(text(&doc), "fallback:1");
    no_errors(&reports);
}
#[test]
fn site_module_failure_is_not_rewritten_as_a_cdn_package() {
    let mut r = runtime(
        r#"<p>before</p><script>import('/_app/start.js').catch(e=>document.querySelector('p').textContent='failed:'+String(e).includes('/_app/start.js'))</script>"#,
        vec![],
    );
    let (doc, reports) = finish(&mut r, 1);
    assert_eq!(text(&doc), "failed:true");
    assert_eq!(netfs::requested(), ["https://fixture.test/_app/start.js"]);
    assert!(reports.iter().any(|r| matches!(
        r,
        Report::Resource {
            status: "import-failed",
            ..
        }
    )));
}
#[test]
fn external_classic_and_module_scripts_load_in_source_order() {
    let mut r = runtime(
        r#"<p>before</p><script src='./classic.js' id='classic'></script><script type=module src='./entry.js'></script>"#,
        vec![
            (
                "https://fixture.test/path/classic.js",
                "if(document.currentScript.getAttribute('id')!=='classic')throw Error('current script');globalThis.order=['classic'];",
            ),
            (
                "https://fixture.test/path/entry.js",
                "if(document.currentScript!==null)throw Error('module current');order.push('module');document.querySelector('p').textContent=order.join(',');",
            ),
        ],
    );
    let (doc, reports) = finish(&mut r, 1);
    assert_eq!(text(&doc), "classic,module");
    no_errors(&reports);
}
#[test]
fn download_wait_is_separate_from_the_250ms_javascript_allowance() {
    let mut r = runtime(
        r#"<p>before</p><script type=module>import{x}from './delayed.js';document.querySelector('p').textContent=x;</script>"#,
        vec![(
            "https://fixture.test/path/delayed.js",
            "export const x='ready after wait';",
        )],
    );
    netfs::delay("https://fixture.test/path/delayed.js", 280);
    let started = Instant::now();
    let (doc, reports) = finish(&mut r, 1);
    assert!(started.elapsed() > Duration::from_millis(250));
    assert_eq!(text(&doc), "ready after wait");
    no_errors(&reports);
}
#[test]
fn navigation_cancels_old_downloads_and_pending_imports() {
    let mut r = runtime(
        r#"<p>old</p><script>import('./old.js').then(()=>{throw Error('old realm resumed')})</script>"#,
        vec![("https://fixture.test/path/old.js", "export const x=1;")],
    );
    netfs::delay("https://fixture.test/path/old.js", 100000);
    let waiting = Instant::now();
    while netfs::active() != 1 && waiting.elapsed() < Duration::from_secs(2) {
        r.poll();
        std::thread::yield_now();
    }
    assert_eq!(netfs::active(), 1);
    begin(
        &mut r,
        2,
        "<p>new</p><script>if(document.currentScript===null)throw Error('scope')</script>",
    );
    let (doc, reports) = finish(&mut r, 2);
    assert_eq!(text(&doc), "new");
    assert_eq!(netfs::active(), 0);
    assert!(netfs::discarded().contains(&"https://fixture.test/path/old.js".into()));
    no_errors(&reports);
}
#[test]
fn import_map_exact_and_prefix_entries_resolve_before_cdn_defaults() {
    let mut r = runtime(
        r#"<p>before</p><script type=importmap>{"imports":{"demo":"./mapped.js","lib/":"./lib/"}}</script><script type=module>import{x}from 'demo';import{y}from 'lib/more.js';document.querySelector('p').textContent=x+y</script>"#,
        vec![
            (
                "https://fixture.test/path/mapped.js",
                "export const x='map';",
            ),
            (
                "https://fixture.test/path/lib/more.js",
                "export const y='ped';",
            ),
        ],
    );
    let (doc, reports) = finish(&mut r, 1);
    assert_eq!(text(&doc), "mapped");
    no_errors(&reports);
    assert!(!netfs::requested().iter().any(|u| u.contains("esm.sh")));
}
