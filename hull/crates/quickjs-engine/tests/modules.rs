use hull_quickjs::Engine;
fn drain(engine: &mut Engine) {
    for _ in 0..100 {
        if !engine.execute_pending_job().unwrap() {
            return;
        }
    }
    panic!("job loop");
}
fn engine() -> Engine {
    let mut engine = Engine::new().unwrap();
    engine
        .install_modules(|base, name| {
            if name.starts_with("https://") {
                Ok(name.into())
            } else if let Some(tail) = name.strip_prefix("./") {
                Ok(format!("{}/{tail}", base.rsplit_once('/').unwrap().0))
            } else {
                Err("unsupported test specifier".into())
            }
        })
        .unwrap();
    engine
}
#[test]
fn parser_discovers_only_static_requests_without_execution() {
    let mut e = engine();
    let deps=e.module_dependencies(br#"import {x} from './a.js';export {y} from './b.js';export * from './c.js';const fake="import('wrong')";const regex=/import\('wrong'\)/;import('./later.js');globalThis.mustNotRun=1;"#,"https://test/entry.js").unwrap();
    assert_eq!(deps, ["./a.js", "./b.js", "./c.js"]);
    e.eval(
        b"if(typeof mustNotRun!=='undefined')throw Error('probe executed')",
        "test",
        false,
    )
    .unwrap();
    assert!(
        e.module_dependencies(b"export const = ;", "https://test/bad.js")
            .is_err()
    );
}
#[test]
fn dynamic_import_waits_for_explicit_completion_and_preserves_namespace_cache() {
    let mut e = engine();
    e.eval(b"globalThis.values=[];import('https://test/a.js').then(m=>values.push(m));import('https://test/a.js').then(m=>values.push(m));","https://test/page",false).unwrap();
    drain(&mut e);
    let requests = e.take_import_requests();
    assert_eq!(requests.len(), 2);
    e.eval(
        b"if(values.length)throw Error('ran before download')",
        "check",
        false,
    )
    .unwrap();
    e.add_module("https://test/a.js",b"globalThis.runs=(globalThis.runs||0)+1;export const value=41;export const url=import.meta.url;").unwrap();
    for r in requests {
        e.complete_import(r.id, None).unwrap();
    }
    drain(&mut e);
    e.eval(b"if(values.length!==2||values[0]!==values[1]||values[0].value!==41||runs!==1||values[0].url!=='https://test/a.js')throw Error('module identity/result')","check",false).unwrap();
}
#[test]
fn static_graph_cycles_live_bindings_and_top_level_await_use_quickjs() {
    let mut e = engine();
    e.add_module("https://test/a.js",b"import {read} from './b.js'; export let x=1;export function inc(){x++} await Promise.resolve();export const answer=read();").unwrap();
    e.add_module(
        "https://test/b.js",
        b"import {x,inc} from './a.js';export function read(){inc();return x;}",
    )
    .unwrap();
    e.eval(b"import('https://test/a.js').then(m=>{if(m.answer!==2||m.x!==2)throw Error('live cycle');globalThis.ok=true})","https://test/page",false).unwrap();
    drain(&mut e);
    let r = e.take_import_requests().pop().unwrap();
    e.complete_import(r.id, None).unwrap();
    drain(&mut e);
    e.eval(
        b"if(globalThis.ok!==true)throw Error('not settled')",
        "check",
        false,
    )
    .unwrap();
}
#[test]
fn host_failure_rejects_import_and_attributes_do_not_get_ignored() {
    let mut e = engine();
    e.eval(
        b"globalThis.message='';import('https://test/no.js').catch(e=>message=String(e));",
        "https://test/page",
        false,
    )
    .unwrap();
    drain(&mut e);
    let r = e.take_import_requests().pop().unwrap();
    e.complete_import(r.id, Some("download failed on both providers"))
        .unwrap();
    drain(&mut e);
    e.eval(b"if(!message.includes('both providers'))throw Error('rejection lost');globalThis.attributesFailed=false;import('https://test/data.json',{with:{type:'json'}}).catch(()=>attributesFailed=true);","check",false).unwrap();
    drain(&mut e);
    assert!(e.take_import_requests().is_empty());
    e.eval(
        b"if(!attributesFailed)throw Error('ignored attributes')",
        "check",
        false,
    )
    .unwrap();
}
