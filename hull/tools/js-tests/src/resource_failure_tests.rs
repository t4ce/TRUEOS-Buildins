use crate::{
    javascript::Report,
    netfs,
    resource_tests::{finish, runtime, text},
};
#[test]
fn both_providers_fail_and_the_import_promise_rejects() {
    let mut r = runtime(
        "<p>before</p><script>import('unavailable@1.0.0').catch(e=>document.querySelector('p').textContent=String(e).includes('failed')?'rejected':'wrong')</script>",
        vec![],
    );
    let (doc, reports) = finish(&mut r, 1);
    assert_eq!(text(&doc), "rejected");
    assert_eq!(
        netfs::requested(),
        [
            "https://esm.sh/unavailable@1.0.0?target=es2022&bundle",
            "https://cdn.jsdelivr.net/npm/unavailable@1.0.0/+esm"
        ]
    );
    assert!(!reports.iter().any(|r| matches!(r, Report::JobError { .. })));
}
#[test]
fn module_runtime_error_surfaces_in_diagnostics() {
    let mut r = runtime(
        "<p>before</p><script>import('./broken.js')</script>",
        vec![(
            "https://fixture.test/path/broken.js",
            "throw Error('module startup failed');export const x=1;",
        )],
    );
    let (doc, reports) = finish(&mut r, 1);
    assert_eq!(text(&doc), "before");
    let errors = reports
        .iter()
        .filter_map(|r| {
            if let Report::JobError { error, .. } = r {
                Some(error)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1, "{reports:?}");
    assert!(errors[0].contains("module startup failed"));
}
#[test]
fn unsupported_schemes_reject_without_a_download() {
    let mut r = runtime(
        "<p>before</p><script>Promise.all(['file:///secret','data:text/javascript,export default 1','node:fs','https://user:password@example.test/x'].map(s=>import(s).catch(()=>1))).then(v=>document.querySelector('p').textContent=String(v.reduce((a,b)=>a+b,0)))</script>",
        vec![],
    );
    let (doc, _) = finish(&mut r, 1);
    assert_eq!(text(&doc), "4");
    assert!(netfs::requested().is_empty());
}
#[test]
fn file_count_and_source_size_limits_release_the_initial_snapshot() {
    let imports = (0..65)
        .map(|i| format!("import './m{i}.js';"))
        .collect::<String>();
    let html = format!(
        "<p>before</p><script type=module>{imports}document.querySelector('p').textContent='wrong'</script>"
    );
    let mut r = runtime(&html, vec![]);
    let (doc, reports) = finish(&mut r, 1);
    assert_eq!(text(&doc), "before");
    assert!(netfs::requested().len() <= 64);
    assert!(
        reports
            .iter()
            .any(|r| matches!(r, Report::Evaluated { result: Err(_), .. }))
    );
    drop(r);
    let code = " ".repeat(4 * 1024 * 1024 + 1);
    let mut r = runtime(
        "<p>before</p><script type=module src='./huge.js'></script>",
        vec![("https://fixture.test/path/huge.js", &code)],
    );
    let (doc, reports) = finish(&mut r, 1);
    assert_eq!(text(&doc), "before");
    assert!(
        reports
            .iter()
            .any(|r| matches!(r,Report::Evaluated{result:Err(e),..}if e.contains("byte budget")))
    );
}
#[test]
fn late_pasted_import_can_finish_without_another_render_snapshot() {
    let mut r = runtime(
        "<p>frozen</p>",
        vec![(
            "https://fixture.test/path/late.js",
            "export const answer='late';",
        )],
    );
    let (doc, _) = finish(&mut r, 1);
    assert_eq!(text(&doc), "frozen");
    r.execute_pasted("import('./late.js').then(m=>console.log(m.answer))".into())
        .unwrap();
    let mut logged = false;
    for _ in 0..100 {
        for report in r.poll() {
            if let Report::Console { batch, .. } = report {
                logged |= batch.messages.iter().any(|m| m.text == "late");
            }
        }
        if logged {
            break;
        }
        std::thread::yield_now();
    }
    assert!(logged);
    assert!(r.take_initial_document(1).is_none());
}
#[test]
fn fragments_keep_separate_module_identity() {
    let mut r = runtime(
        "<p>before</p><script>globalThis.runs=0;Promise.all([import('./same.js#a'),import('./same.js#b')]).then(([a,b])=>document.querySelector('p').textContent=String(a!==b&&runs===2))</script>",
        vec![
            (
                "https://fixture.test/path/same.js#a",
                "globalThis.runs++;export const x=1;",
            ),
            (
                "https://fixture.test/path/same.js#b",
                "globalThis.runs++;export const x=1;",
            ),
        ],
    );
    let (doc, _) = finish(&mut r, 1);
    assert_eq!(text(&doc), "true");
}
