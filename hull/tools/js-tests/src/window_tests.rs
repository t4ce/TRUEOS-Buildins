use crate::{
    javascript::{Report, Runtime},
    javascript_dom::{self, SharedWindowMetrics, WindowMetrics},
    scripts::Scripts,
};
use hull_quickjs::Engine;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

fn content_metrics(frame_width: u32, frame_height: u32) -> WindowMetrics {
    let mut viewport = crate::viewport::Viewport::default();
    viewport.resize_with_origin(frame_width, frame_height, 0, 32);
    let (inner_width, inner_height) = viewport.extent();
    WindowMetrics {
        inner_width,
        inner_height,
    }
}

fn fixture() -> (Engine, SharedWindowMetrics) {
    let document = Rc::new(RefCell::new(rp5::parse("<p id='message'>before</p>")));
    let metrics = Rc::new(Cell::new(content_metrics(1280, 752)));
    let mut engine = Engine::new().unwrap();
    javascript_dom::install_with_window(&mut engine, document, metrics.clone()).unwrap();
    (engine, metrics)
}
fn evaluate(engine: &mut Engine, source: &str) {
    engine
        .eval(source.as_bytes(), "window-fixture.js", false)
        .unwrap();
}

#[test]
fn navigator_has_virtual_identity_and_lazy_empty_plugin_collections() {
    let (mut engine,_)=fixture();
    evaluate(&mut engine,r#"
        if(window.navigator!==navigator||clientInformation!==navigator||!(navigator instanceof Navigator))throw Error('navigator identity');
        if(navigator.userAgent!=='TRUEOS net-fetch'||navigator.platform!=='TRUEOS'||navigator.pdfViewerEnabled!==false)throw Error('virtual identity');
        const plugins=navigator.plugins,mimes=navigator.mimeTypes;
        if(plugins!==navigator.plugins||mimes!==navigator.mimeTypes||!(plugins instanceof PluginArray)||!(mimes instanceof MimeTypeArray))throw Error('collection identity');
        if(plugins.length!==0||mimes.length!==0||plugins[0]!==undefined||plugins['Shockwave Flash']!==undefined)throw Error('phantom plugin');
        if(plugins.item(0)!==null||plugins.namedItem('Shockwave Flash')!==null||mimes.namedItem('video/mp4')!==null||[...plugins].length||[...mimes].length)throw Error('empty collection');
        plugins.refresh();
        for(const name of ['serviceWorker','userAgentData','mediaDevices','gpu','geolocation','hardwareConcurrency','onLine'])if(name in navigator)throw Error('invented capability '+name);
        let failed=false;try{Object.getOwnPropertyDescriptor(Navigator.prototype,'userAgent').get.call({})}catch(e){failed=e instanceof TypeError}if(!failed)throw Error('brand');
        failed=false;try{plugins.namedItem()}catch(e){failed=e instanceof TypeError}if(!failed)throw Error('required argument');
        failed=false;try{new Navigator()}catch(e){failed=e instanceof TypeError}if(!failed)throw Error('constructor');
    "#);
}

#[test]
fn virtual_navigator_is_fresh_on_navigation() {
    let mut runtime=Runtime::start().unwrap();runtime.update_viewport(1280,720);
    begin(&mut runtime,301,"<script>navigator.previousPage=1;navigator.plugins.previousPage=1;</script>");
    let (_,reports)=snapshot(&mut runtime,301);assert_success(&reports);
    begin(&mut runtime,302,"<script>if(navigator.previousPage!==undefined||navigator.plugins.previousPage!==undefined)throw Error('old navigator');</script>");
    let (_,reports)=snapshot(&mut runtime,302);assert_success(&reports);
}
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
    let mut reports = Vec::new();
    let waiting=std::time::Instant::now();
        while waiting.elapsed()<std::time::Duration::from_secs(3) {
            std::thread::yield_now();
        reports.extend(runtime.poll());
        if let Some(document) = runtime.take_initial_document(generation) {
            reports.extend(runtime.poll());return (document, reports);
        }
    }
    panic!("window-backed page did not initialize: {reports:?}");
}
fn assert_success(reports: &[Report]) {
    for report in reports {
        match report {
            Report::Evaluated { result, .. } | Report::Pasted { result, .. } => {
                assert!(result.is_ok(), "{report:?}")
            }
            Report::JobError { .. } => panic!("{report:?}"),
            _ => {}
        }
    }
}

#[test]
fn window_aliases_the_page_global_and_reads_live_content_dimensions() {
    let (mut engine, metrics) = fixture();
    evaluate(
        &mut engine,
        r#"
        if(window!==globalThis||self!==window||window.window!==window||window.self!==window)throw Error('global identity');
        if(window.document!==document)throw Error('document identity');
        window.businessState={value:42};
        if(globalThis.businessState.value!==42||self.businessState!==businessState)throw Error('global writes');
        if(innerWidth!==1280||innerHeight!==720||window.innerHeight===752)throw Error('content viewport includes chrome');
        for(const name of ['devicePixelRatio','outerWidth','outerHeight','screen','screenX','screenY','closed','fetch','XMLHttpRequest','setTimeout','require','process','std','os','__hullDom']) {
            if(typeof window[name]!=='undefined')throw Error('unsupported capability exposed: '+name);
        }
        if(typeof document.hasFocus!=='undefined')throw Error('stubbed focus exposed');
        if(typeof window.addEventListener!=='function'||typeof document.addEventListener!=='function')throw Error('DOM event targets missing');
    "#,
    );
    // Hull's resize callback supplies its actual content extent, not the full
    // native frame: a 640x400 frame reserves the existing 32-unit chrome row.
    metrics.set(content_metrics(640, 400));
    evaluate(
        &mut engine,
        "if(window.innerWidth!==640||self.innerHeight!==368)throw Error('stale viewport')",
    );
    metrics.set(content_metrics(0, 0));
    evaluate(
        &mut engine,
        "if(innerWidth!==0||innerHeight!==0)throw Error('empty viewport')",
    );
}

#[test]
fn replaceable_properties_shadow_normally_and_window_keeps_the_original_global() {
    let (mut engine, metrics) = fixture();
    evaluate(
        &mut engine,
        r#"
        const actualWindow=window;
        innerWidth=17;innerHeight='script value';self='replacement';
        if(innerWidth!==17||innerHeight!=='script value'||self!=='replacement')throw Error('replaceable attributes');
        if(window!==actualWindow||actualWindow.document!==document)throw Error('window changed');
        const descriptor=Object.getOwnPropertyDescriptor(actualWindow,'window');
        if(descriptor.configurable||descriptor.set!==undefined)throw Error('window is writable');
        let refused=false;try{(()=>{'use strict';window={}})()}catch(error){refused=error instanceof TypeError}
        if(!refused||window!==actualWindow)throw Error('window assignment accepted');
        globalThis={};
        if(window!==actualWindow||window.window!==actualWindow||window.document!==document)throw Error('window depends on replaced globalThis');
    "#,
    );
    metrics.set(content_metrics(640, 400));
    evaluate(
        &mut engine,
        "if(innerWidth!==17||innerHeight!=='script value'||self!=='replacement')throw Error('host overwrote page replacements')",
    );
}

#[test]
fn actor_uses_native_viewport_updates_without_redraw_and_resets_globals_on_navigation() {
    let mut runtime = Runtime::start().unwrap();
    let initial = content_metrics(1280, 752);
    runtime.update_viewport(initial.inner_width, initial.inner_height);
    begin(
        &mut runtime,
        41,
        "<p>before</p><script>window.firstPage=true;document.querySelector('p').textContent=`${innerWidth}x${innerHeight}`</script>",
    );
    let (frozen, reports) = snapshot(&mut runtime, 41);
    assert_success(&reports);
    let p = frozen
        .nodes
        .iter()
        .enumerate()
        .find_map(|(id, _)| (frozen.name(id) == "p").then_some(id))
        .unwrap();
    assert_eq!(frozen.text_content(p), "1280x720");

    let resized = content_metrics(640, 400);
    runtime.update_viewport(resized.inner_width, resized.inner_height);
    runtime.execute_pasted("if(window.innerWidth!==640||self.innerHeight!==368)throw Error('resize');document.querySelector('p').textContent='live only';console.log(`${innerWidth}x${innerHeight}`)".into()).unwrap();
    let mut output = Vec::new();
    let mut reports = Vec::new();
    let waiting=std::time::Instant::now();
    while waiting.elapsed()<std::time::Duration::from_secs(2) {
        std::thread::yield_now();
        let turn = runtime.poll();
        for report in &turn {
            if let Report::Console { batch, .. } = report {
                output.extend(batch.messages.iter().map(|m| m.text.clone()));
            }
        }
        reports.extend(turn);
        if reports.iter().any(|r| matches!(r, Report::Pasted { .. })) {
            break;
        }
    }
    assert_success(&reports);
    assert_eq!(output, ["640x368"]);
    assert_eq!(frozen.text_content(p), "1280x720");
    assert!(
        runtime.take_initial_document(41).is_none(),
        "resize or DOM mutation generated another render snapshot"
    );

    begin(
        &mut runtime,
        42,
        "<p>next</p><script>if(typeof window.firstPage!=='undefined')throw Error('old page globals');if(innerWidth!==640||innerHeight!==368)throw Error('lost current viewport')</script>",
    );
    let (_, reports) = snapshot(&mut runtime, 42);
    assert_success(&reports);
}

#[test]
fn dom_without_a_native_window_producer_keeps_window_capabilities_absent() {
    let mut engine = Engine::new().unwrap();
    let document = Rc::new(RefCell::new(rp5::parse("<p>no native window</p>")));
    javascript_dom::install(&mut engine, document).unwrap();
    evaluate(
        &mut engine,
        "if(typeof document==='undefined')throw Error('DOM missing');for(const name of ['window','self','innerWidth','innerHeight','navigator','Navigator'])if(typeof globalThis[name]!=='undefined')throw Error('fabricated window state: '+name)",
    );

    let mut runtime = Runtime::start().unwrap();
    begin(
        &mut runtime,
        43,
        "<script>if(typeof window!=='undefined'||typeof self!=='undefined')throw Error('fabricated actor window')</script>",
    );
    let (_, reports) = snapshot(&mut runtime, 43);
    assert_success(&reports);
}
