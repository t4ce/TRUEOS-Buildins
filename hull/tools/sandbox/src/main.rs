//! Host-only replay of production Hull code. No kernel, Winit window or GPU.
#![allow(dead_code)]
extern crate self as trueos;
pub use tokio;

use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

// Data-only window geometry facade for the production hover projection. Replay
// never registers these regions with a window or invokes a kernel UI service.
pub mod ui4_winit {
    #[derive(Clone, Copy, Debug)]
    pub struct ButtonRegion {
        pub x: i32,
        pub y: i32,
        pub width: u32,
        pub height: u32,
        pub clip_x: i32,
        pub clip_y: i32,
        pub clip_width: u32,
        pub clip_height: u32,
        pub border_rgba: u32,
    }
}

#[path = "../../../src/buttons.rs"]
mod buttons;

#[path = "../../../src/resource_url.rs"]
mod resource_url;
#[path = "../../../src/css.rs"]
mod css;
#[path = "../../../src/javascript.rs"]
mod javascript;
#[path = "../../../src/javascript_dom.rs"]
mod javascript_dom;
#[path = "../../../src/layout.rs"]
mod layout;
#[path = "../../../src/links.rs"]
mod links;
#[path = "../../../src/scripts.rs"]
mod scripts;
#[path = "../../../src/selectors.rs"]
mod selectors;
#[path = "../../../src/shellio.rs"]
mod shellio;
#[path = "../../../src/viewport.rs"]
mod viewport;

pub mod logl {
    use std::cell::RefCell;
    pub mod level {
        pub const INFO: u32 = 1;
        pub const ERROR: u32 = 2;
    }
    thread_local! { static LOGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }
    pub fn log(_: u32, message: std::fmt::Arguments<'_>) {
        LOGS.with(|logs| {
            let mut logs = logs.borrow_mut();
            if logs.len() < 2048 {
                logs.push(message.to_string());
            }
        });
    }
    pub fn take() -> Vec<String> {
        LOGS.with(|logs| logs.borrow_mut().drain(..).collect())
    }
}

// The CSS collector sees completed local tickets only. This module never opens
// a socket: absent/failed captures are errors, not live-network fallbacks.
pub mod netfs {
    use super::*;
    use std::cell::RefCell;
    #[derive(Default)]
    struct Network {
        resources: HashMap<String, Vec<u8>>,
        tickets: HashMap<u32, String>,
        requested: Vec<String>,
        next: u32,
    }
    thread_local! { static NET: RefCell<Network> = RefCell::new(Network::default()); }
    pub fn install(resources: HashMap<String, Vec<u8>>) {
        NET.with(|net| {
            *net.borrow_mut() = Network {
                resources,
                ..Network::default()
            }
        });
    }
    pub fn fetch_bytes(url: &[u8]) -> Result<u32, i32> {
        let url = std::str::from_utf8(url).map_err(|_| -1)?.to_owned();
        NET.with(|net| {
            let mut net = net.borrow_mut();
            net.next += 1;
            let id = net.next;
            net.requested.push(url.clone());
            net.tickets.insert(id, url);
            Ok(id)
        })
    }
    pub fn fetch_bytes_happy_eyeballs(url: &[u8]) -> Result<u32, i32> {
        fetch_bytes(url)
    }
    pub fn fetch_bytes_result_len(id: u32) -> Result<usize, i32> {
        NET.with(|net| {
            let net = net.borrow();
            net.tickets
                .get(&id)
                .and_then(|url| net.resources.get(url))
                .map(Vec::len)
                .ok_or(-2)
        })
    }
    pub fn fetch_bytes_read(id: u32) -> Result<Vec<u8>, i32> {
        NET.with(|net| {
            let net = net.borrow();
            net.tickets
                .get(&id)
                .and_then(|url| net.resources.get(url))
                .cloned()
                .ok_or(-2)
        })
    }
    pub fn fetch_bytes_discard(id: u32) -> i32 {
        NET.with(|net| {
            net.borrow_mut().tickets.remove(&id);
        });
        0
    }
    pub fn requested() -> Vec<String> {
        NET.with(|net| net.borrow().requested.clone())
    }
}

#[derive(Debug)]
struct Options {
    input: PathBuf,
    out: PathBuf,
    inspect: bool,
    no_js: bool,
    no_svg: bool,
    external: bool,
    modules: bool,
    no_window: bool,
    width: u32,
    height: u32,
    turn_ms: u64,
    only_node: Option<usize>,
    replacements: HashMap<usize, PathBuf>,
    prelude: Option<PathBuf>,
}
impl Options {
    fn parse() -> Result<Self, String> {
        let mut result = Self {
            input: PathBuf::new(),
            out: PathBuf::new(),
            inspect: false,
            no_js: false,
            no_svg: false,
            external: false,
            modules: false,
            no_window: false,
            width: 1280,
            height: 720,
            turn_ms: 20,
            only_node: None,
            replacements: HashMap::new(),
            prelude: None,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--inspect" => result.inspect = true,
                "--no-js" => result.no_js = true,
                "--no-svg" => result.no_svg = true,
                "--external" => result.external = true,
                "--modules" => result.modules = true,
                "--no-window" => result.no_window = true,
                _ => {
                    let value = args
                        .next()
                        .ok_or_else(|| format!("missing value for {arg}"))?;
                    match arg.as_str() {
                        "--input" => result.input = value.into(),
                        "--out" => result.out = value.into(),
                        "--width" => result.width = value.parse().map_err(|_| "invalid width")?,
                        "--height" => {
                            result.height = value.parse().map_err(|_| "invalid height")?
                        }
                        "--turn-ms" => {
                            result.turn_ms = value.parse().map_err(|_| "invalid turn delay")?
                        }
                        "--only-node" => {
                            result.only_node = Some(value.parse().map_err(|_| "invalid node")?)
                        }
                        "--prelude" => result.prelude = Some(value.into()),
                        "--replace" => {
                            let (node, file) = value
                                .split_once('=')
                                .ok_or("replacement must be NODE=FILE")?;
                            result.replacements.insert(
                                node.parse().map_err(|_| "invalid replacement node")?,
                                file.into(),
                            );
                        }
                        _ => return Err(format!("unknown option {arg}")),
                    }
                }
            }
        }
        if result.input.as_os_str().is_empty() || result.out.as_os_str().is_empty() {
            return Err("--input and --out are required".into());
        }
        if result.width == 0
            || result.height == 0
            || result.width > 8192
            || result.height > 8192
            || result.turn_ms > 100
        {
            return Err("viewport must be 1..8192; turn-ms must be 0..100".into());
        }
        Ok(result)
    }
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    if fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len()
        > limit
    {
        return Err(format!("{} exceeds {limit} bytes", path.display()));
    }
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}
fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).map_err(|e| e.to_string())
}
fn captured_resources(input: &Path, manifest: &Value) -> Result<HashMap<String, Vec<u8>>, String> {
    let mut resources = HashMap::new();
    let root = input.canonicalize().map_err(|e| e.to_string())?;
    if let Some(entries) = manifest.get("resources").and_then(Value::as_object) {
        if entries.len() > 128 {
            return Err("capture contains more than 128 resources".into());
        }
        let mut total = 0;
        for (url, entry) in entries {
            if entry.get("status").and_then(Value::as_str) != Some("ok") {
                continue;
            }
            let name = entry
                .get("path")
                .and_then(Value::as_str)
                .ok_or("resource has no path")?;
            let file = input.join(name).canonicalize().map_err(|e| e.to_string())?;
            if !file.starts_with(&root) {
                return Err("resource path escapes capture directory".into());
            }
            let bytes = read_bounded(&file, 4 * 1024 * 1024)?;
            total += bytes.len();
            if total > 16 * 1024 * 1024 {
                return Err("captured resources exceed 16 MiB".into());
            }
            resources.insert(url.clone(), bytes);
        }
    }
    Ok(resources)
}

fn source_inventory(sources: &[scripts::ScriptSource], out: &Path) -> Result<Vec<Value>, String> {
    if sources.len() > 512 {
        return Err("page contains more than 512 collected script sources".into());
    }
    fs::create_dir_all(out.join("scripts")).map_err(|e| e.to_string())?;
    sources.iter().enumerate().map(|(index, source)| {
        let file = source.bytes.as_ref().map(|_| format!("scripts/{:03}-node-{}.js", index + 1, source.nodes.first().copied().unwrap_or(0)));
        if let (Some(file), Some(bytes)) = (&file, &source.bytes) { fs::write(out.join(file), bytes).map_err(|e| e.to_string())?; }
        Ok(json!({"index":index+1,"nodes":source.nodes,"origin":source.origin,"type":source.kind,
            "file":file,"bytes":source.bytes.as_ref().map(Vec::len),"status":"not-started"}))
    }).collect()
}
fn record(report: javascript::Report) -> Value {
    use javascript::Report;
    match report {
        Report::Resource {
            url,
            status,
            detail,
            ..
        } => json!({"event":"resource","url":url,"status":status,"detail":detail}),
        Report::Online => json!({"event":"online"}),
        Report::Page {
            generation,
            sources,
        } => json!({"event":"page","generation":generation,"sources":sources}),
        Report::Evaluated {
            node,
            origin,
            result,
            ..
        } => {
            json!({"event":"script","node":node,"origin":origin,"status":if result.is_ok(){"executed"}else{"failed"},"error":result.err()})
        }
        Report::Skipped { origin, reason, .. } => {
            json!({"event":"script","origin":origin,"status":"skipped","reason":reason})
        }
        Report::JobError { error, .. } => json!({"event":"job-error","error":error}),
        Report::Console { batch, .. } => {
            json!({"event":"console","messages":batch.messages.iter().map(|m|json!({"level":m.level.as_str(),"text":m.text})).collect::<Vec<_>>(),"dropped":batch.dropped})
        }
        Report::Initialized {
            capped,
            remaining_sources,
            ..
        } => json!({"event":"initialized","capped":capped,"remaining_sources":remaining_sources}),
        Report::BudgetExhausted { .. } => json!({"event":"budget-exhausted"}),
        Report::Input { kind, result, .. } => {
            json!({"event":"input","kind":kind,"error":result.err()})
        }
        Report::Pasted { result, .. } => json!({"event":"pasted","error":result.err()}),
    }
}
fn script_sources(
    options: &Options,
    sources: &[scripts::ScriptSource],
    resources: &HashMap<String, Vec<u8>>,
) -> Result<(Vec<scripts::ScriptSource>, Vec<String>), String> {
    let mut selected = Vec::new();
    let mut experiments = Vec::new();
    if let Some(file) = &options.prelude {
        selected.push(scripts::ScriptSource {
            nodes: vec![0],
            origin: file.display().to_string(),
            kind: "text/javascript".into(),
            bytes: Some(read_bounded(file, 4 * 1024 * 1024)?),
        });
        experiments.push(format!("prelude: {} (reported as node 0)", file.display()));
    }
    if let Some(node) = options.only_node {
        if !sources.iter().any(|s| s.nodes.contains(&node)) {
            return Err(format!("script node {node} is absent from this capture"));
        }
        experiments.push(format!("isolated script node {node}"));
    }
    for node in options.replacements.keys() {
        if !sources.iter().any(|s| s.nodes.contains(node)) {
            return Err(format!("replacement node {node} is absent"));
        }
    }
    for source in sources {
        if options
            .only_node
            .is_some_and(|node| !source.nodes.contains(&node))
        {
            continue;
        }
        let mut source = source.clone();
        if let Some(file) = source
            .nodes
            .iter()
            .find_map(|node| options.replacements.get(node))
        {
            source.bytes = Some(read_bounded(file, 4 * 1024 * 1024)?);
            experiments.push(format!(
                "replace nodes {:?}: {}",
                source.nodes,
                file.display()
            ));
        } else if source.bytes.is_none() && options.external {
            source.bytes = resources.get(&source.origin).cloned();
        }
        selected.push(source);
    }
    if options.external {
        experiments
            .push("captured external JavaScript enabled without the module graph loader".into());
    }
    Ok((selected, experiments))
}

fn run(options: Options) -> Result<(), String> {
    let manifest: Value = serde_json::from_slice(&read_bounded(
        &options.input.join("capture.json"),
        512 * 1024,
    )?)
    .map_err(|e| e.to_string())?;
    let page_url = manifest
        .get("url")
        .and_then(Value::as_str)
        .ok_or("capture.json needs url")?;
    let url = url::Url::parse(page_url).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "https" | "http") {
        return Err("page URL must be HTTP(S)".into());
    }
    let html = read_bounded(&options.input.join("page.html"), 4 * 1024 * 1024)?;
    fs::create_dir_all(&options.out).map_err(|e| e.to_string())?;
    // Do not leave a previous successful SVG beside a failed or JS-only run.
    for name in [
        "layout.svg",
        "storage.json",
        "layout.json",
        "report.json",
        "trace.jsonl",
        "console.log",
    ] {
        let _ = fs::remove_file(options.out.join(name));
    }
    let document = rp5::parse(&String::from_utf8_lossy(&html));
    let parse_nodes = document.nodes.len();
    let parse_errors = document.errors.len();
    let mut extracted = scripts::Scripts::default();
    extracted.extract(&document, page_url);
    let resources = captured_resources(&options.input, &manifest)?;
    netfs::install(resources.clone());
    let mut inventory = source_inventory(&extracted.sources, &options.out)?;
    write_json(&options.out.join("scripts.json"), &json!(inventory))?;
    let started = Instant::now();
    let mut trace = Vec::new();
    let mut turns = 0usize;
    let mut cookies = "[]".to_owned();
    let (frozen, experiments) = if options.inspect || options.no_js {
        for source in &mut inventory {
            source["status"] = json!(if options.inspect {
                "not-run"
            } else {
                "disabled"
            });
        }
        (document, Vec::new())
    } else {
        let (sources, experiments) = script_sources(&options, &extracted.sources, &resources)?;
        // Dump the actual selected/replaced source bytes too, preserving raw originals.
        let mut selected = source_inventory(&sources, &options.out.join("admitted"))?;
        let mut runtime = javascript::Runtime::start()?;
        if options.modules {
            runtime.enable_resource_loading();
        }
        if !options.no_window {
            runtime.update_viewport(options.width, options.height);
        }
        runtime.begin_page(1, page_url, sources, document);
        let frozen = loop {
            let turn_start = Instant::now();
            for report in runtime.poll() {
                let mut event = record(report);
                event["turn"] = json!(turns);
                event["elapsed_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
                trace.push(event);
            }
            let active_turn_ms = turn_start.elapsed().as_secs_f64() * 1000.0;
            trace.push(json!({"event":"turn","turn":turns,"host_turn_ms":active_turn_ms}));
            turns += 1;
            if let Some(frozen) = runtime.take_initial_document(1) {
                break frozen;
            }
            if turns >= 16384 || started.elapsed() > Duration::from_secs(7) {
                return Err("initialization did not produce a render snapshot".into());
            }
            if options.turn_ms != 0 {
                std::thread::sleep(Duration::from_millis(options.turn_ms));
            }
        };
        for event in &trace {
            if event["event"] != "script" {
                continue;
            }
            let matched = if let Some(node) = event["node"].as_u64() {
                selected.iter_mut().find(|source| {
                    source["nodes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|n| n.as_u64() == Some(node))
                })
            } else {
                selected.iter_mut().find(|source| {
                    source["origin"] == event["origin"] && source["status"] == "not-started"
                })
            };
            if let Some(source) = matched {
                source["status"] = event["status"].clone();
                source["error"] = event["error"].clone();
                source["reason"] = event["reason"].clone();
            }
        }
        for source in &mut inventory {
            if let Some(admitted) = selected.iter().find(|s| s["nodes"] == source["nodes"]) {
                source["status"] = admitted["status"].clone();
                source["error"] = admitted["error"].clone();
                source["reason"] = admitted["reason"].clone();
            } else {
                source["status"] = json!("excluded");
            }
        }
        write_json(&options.out.join("admitted/scripts.json"), &json!(selected))?;
        cookies = runtime.storage_snapshot();
        (frozen, experiments)
    };
    let javascript_wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    let resource_request_count = netfs::requested().len();
    let mut css = css::Css::default();
    css.parse_document(&frozen, page_url);
    for _ in 0..32 {
        if !css.is_pending() {
            break;
        }
        css.poll_downloads();
    }
    css.freeze();
    let css_requests = netfs::requested()
        .into_iter()
        .skip(resource_request_count)
        .collect::<Vec<_>>();
    let mut layout_stats = Value::Null;
    if !options.inspect && !options.no_svg {
        let start = Instant::now();
        let export = layout::compute(&frozen, &css, (options.width, options.height))
            .map_err(|e| e.to_string())?;
        if export.svg.len() > 8 * 1024 * 1024 || export.json.len() > 32 * 1024 * 1024 {
            return Err("layout export exceeds artifact limit".into());
        }
        fs::write(options.out.join("layout.svg"), &export.svg).map_err(|e| e.to_string())?;
        fs::write(options.out.join("storage.json"), &cookies).map_err(|e| e.to_string())?;
        fs::write(options.out.join("layout.json"), &export.json).map_err(|e| e.to_string())?;
        layout_stats = json!({"ms":start.elapsed().as_secs_f64()*1000.0,"boxes":export.line_rects.len(),"text_runs":export.text_runs.len(),"images":export.image_boxes.len(),"extent":export.line_extent});
    }
    let mut console = String::new();
    for event in &trace {
        if event["event"] == "console" {
            for message in event["messages"].as_array().unwrap() {
                console.push_str(&format!(
                    "[{}] {}\n",
                    message["level"].as_str().unwrap(),
                    message["text"].as_str().unwrap()
                ));
            }
        }
        if let Some(error) = event["error"].as_str() {
            console.push_str(&format!(
                "[error] node={} {}\n{error}\n",
                event["node"],
                event["origin"].as_str().unwrap_or("")
            ));
        }
        if event["event"] == "resource" {
            console.push_str(&format!(
                "[resource] {} {} {}\n",
                event["status"].as_str().unwrap_or(""),
                event["url"].as_str().unwrap_or(""),
                event["detail"].as_str().unwrap_or("")
            ));
        }
    }
    fs::write(options.out.join("console.log"), console).map_err(|e| e.to_string())?;
    fs::write(
        options.out.join("trace.jsonl"),
        trace
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect::<String>(),
    )
    .map_err(|e| e.to_string())?;
    let initialized = trace
        .iter()
        .find(|e| e["event"] == "initialized")
        .cloned()
        .unwrap_or(Value::Null);
    let failed = inventory.iter().filter(|s| s["status"] == "failed").count();
    let report = json!({"url":page_url,"mode":if options.inspect{"inspect"}else if options.no_js{"static"}else{"replay"},
        "viewport":[options.width,options.height],"turn_ms":options.turn_ms,"window":!options.no_window,
        "active_budget_ms":250,"initial_wall_window_ms":250,"module_loading":options.modules,"resource_wait_budget_ms":if options.modules{5000}else{0},"javascript_wall_ms":javascript_wall_ms,"turns":turns,
        "parse":{"nodes":parse_nodes,"errors":parse_errors},"initialization":initialized,"experiments":experiments,
        "scripts":inventory,"css_requests":css_requests,"resources":trace.iter().filter(|e|e["event"]=="resource").collect::<Vec<_>>(),"job_errors":trace.iter().filter(|e|e["event"]=="job-error").map(|e|&e["error"]).collect::<Vec<_>>(),"layout":layout_stats,"diagnostics":logl::take(),
        "notes":["Replay has no live network, kernel, GPU, media or image decoding.","SVG is Hull's diagnostic text/box export, not a browser screenshot.","host_turn_ms includes actor and snapshot work; it is not an exact QuickJS CPU counter."]});
    write_json(&options.out.join("report.json"), &report)?;
    println!(
        "{} sources; {failed} failed; capped={}; artifacts={}",
        extracted.sources.len(),
        report["initialization"]["capped"],
        options.out.display()
    );
    Ok(())
}
// Capture-side parser/resolver utilities. They never evaluate module bodies or
// make network requests; Python's capture command owns all resource downloads.
fn module_tool(arguments: &[String]) -> Result<(), String> {
    let mode = arguments.get(1).ok_or("missing module command")?;
    let base = arguments.get(3).ok_or("missing module base URL")?;
    let mut policy = javascript::module_loader::Policy::default();
    if let Some(capture) = arguments.get(4) {
        let capture = Path::new(capture);
        let manifest: Value =
            serde_json::from_slice(&read_bounded(&capture.join("capture.json"), 512 * 1024)?)
                .map_err(|e| e.to_string())?;
        let page = manifest["url"].as_str().ok_or("Capture has no URL")?;
        let html = read_bounded(&capture.join("page.html"), 4 * 1024 * 1024)?;
        policy.document_maps(&rp5::parse(&String::from_utf8_lossy(&html)), page)?;
    }
    if mode == "--module-resolve" {
        // Positional order is command, specifier, base URL.
        let urls = policy.candidates(base, arguments.get(2).ok_or("missing specifier")?)?;
        println!("{}", json!({"candidates":urls}));
        return Ok(());
    }
    let path = Path::new(arguments.get(2).ok_or("missing module source path")?);
    let source = read_bounded(path, 4 * 1024 * 1024)?;
    let mut engine = hull_quickjs::Engine::new()?;
    let names = engine.module_dependencies(&source, base)?;
    let deps = names
        .into_iter()
        .map(|name| {
            policy
                .candidates(base, &name)
                .map(|urls| json!({"specifier":name,"candidates":urls}))
        })
        .collect::<Result<Vec<_>, String>>()?;
    println!("{}", json!({"dependencies":deps}));
    Ok(())
}
fn main() {
    let arguments = std::env::args().collect::<Vec<_>>();
    if arguments
        .get(1)
        .is_some_and(|arg| matches!(arg.as_str(), "--module-inspect" | "--module-resolve"))
    {
        if let Err(error) = module_tool(&arguments) {
            eprintln!("hull-sandbox: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = Options::parse().and_then(run) {
        eprintln!("hull-sandbox: {error}");
        std::process::exit(1);
    }
}
