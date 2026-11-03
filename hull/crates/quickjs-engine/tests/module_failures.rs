use hull_quickjs::Engine;
fn drain(e: &mut Engine) {
    for _ in 0..100 {
        if !e.execute_pending_job().unwrap() {
            return;
        }
    }
    panic!("pending jobs");
}
#[test]
fn parser_dependency_metadata_never_calls_inherited_array_setters() {
    let mut e = Engine::new().unwrap();
    e.eval(b"globalThis.polluted=false;Object.defineProperty(Array.prototype,'0',{set(){polluted=true},configurable:true});","test",false).unwrap();
    assert_eq!(
        e.module_dependencies(b"import './dependency.js';", "https://test/entry.js")
            .unwrap(),
        ["./dependency.js"]
    );
    e.eval(b"delete Array.prototype[0];if(polluted)throw Error('parser metadata executed guest setter')","test",false).unwrap();
}
#[test]
fn unhandled_module_runtime_rejections_are_visible_and_caught_ones_are_suppressed() {
    let mut e = Engine::new().unwrap();
    e.install_modules(|_, name| Ok(name.into())).unwrap();
    e.add_module(
        "https://test/broken.js",
        b"throw Error('module startup failed');export const x=1;",
    )
    .unwrap();
    e.eval(
        b"import('https://test/broken.js');",
        "https://test/page",
        false,
    )
    .unwrap();
    drain(&mut e);
    let request = e.take_import_requests().pop().unwrap();
    e.complete_import(request.id, None).unwrap();
    drain(&mut e);
    let errors = e.take_unhandled_rejections();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("module startup failed"));
    e.eval(
        b"import('https://test/broken.js').catch(()=>{});",
        "https://test/page",
        false,
    )
    .unwrap();
    drain(&mut e);
    let request = e.take_import_requests().pop().unwrap();
    e.complete_import(request.id, None).unwrap();
    drain(&mut e);
    assert!(e.take_unhandled_rejections().is_empty());
}
