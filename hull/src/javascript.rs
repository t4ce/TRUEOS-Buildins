//! Cooperative page initialization and explicit console execution on one local task.
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use hull_quickjs::{ConsoleBatch, Engine};
use serde_json::{Value, json};
use trueos::tokio::{
    runtime,
    sync::{mpsc, watch},
    task::{self, JoinHandle, LocalSet},
};

use crate::javascript_dom::{
    self, SharedCurrentScript, SharedDocument, SharedWindowMetrics, WindowMetrics,
};
use crate::scripts::ScriptSource;
#[path = "module_loader.rs"]
pub(super) mod module_loader;
use module_loader::{Loader, Prepared};

const PAGE_EXECUTION_BUDGET: Duration = Duration::from_millis(250);
const INITIAL_RENDER_WAIT: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub(super) enum Report {
    Online,
    Resource {
        generation: u64,
        url: String,
        status: &'static str,
        detail: String,
    },
    Page {
        generation: u64,
        sources: usize,
    },
    Evaluated {
        generation: u64,
        origin: String,
        node: usize,
        result: Result<(), String>,
    },
    Skipped {
        generation: u64,
        origin: String,
        reason: &'static str,
    },
    JobError {
        generation: u64,
        error: String,
    },
    Input {
        generation: u64,
        kind: String,
        result: Result<(), String>,
    },
    Console {
        generation: u64,
        batch: ConsoleBatch,
    },
    Pasted {
        generation: u64,
        result: Result<(), String>,
    },
    Initialized {
        generation: u64,
        capped: bool,
        remaining_sources: usize,
    },
    BudgetExhausted {
        generation: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EnqueueResult {
    Accepted,
    Dormant,
    Stale,
    Overflow,
    Invalid,
}
const INPUT_CAPACITY: usize = 256;
const INPUT_BYTES: usize = 16 * 1024;
#[derive(Clone, Copy, Default)]
struct InputLoss {
    count: u64,
    sequence: u64,
}
struct PendingReset {
    dropped: u64,
    sequence: u64,
}
struct InputPacket {
    revision: u64,
    generation: u64,
    kind: String,
    bytes: Vec<u8>,
}
struct PastedScript {
    revision: u64,
    generation: u64,
    source: String,
}

#[derive(Clone)]
struct Page {
    revision: u64,
    generation: u64,
    url: String,
    sources: Arc<[ScriptSource]>,
    requested: bool,
    active: bool,
    document: Option<SharedDocument>,
    current_script: SharedCurrentScript,
    load_resources: bool,
    window: Option<SharedWindowMetrics>,
    initial_started: Option<Instant>,
}

#[derive(Default)]
struct InitialRender {
    generation: u64,
    pending: bool,
    snapshot: Option<rp5::Document>,
}

pub(super) struct Runtime {
    load_resources: bool,
    local: LocalSet,
    runtime: runtime::Runtime,
    task: JoinHandle<()>,
    pages: watch::Sender<Option<Page>>,
    ticks: watch::Sender<u64>,
    reports: mpsc::Receiver<Report>,
    input: mpsc::Sender<InputPacket>,
    pasted: Rc<RefCell<VecDeque<PastedScript>>>,
    losses: watch::Sender<InputLoss>,
    sequence: u64,
    revision: u64,
    tick: u64,
    initial: Rc<RefCell<InitialRender>>,
    exhausted: Rc<Cell<bool>>,
    window: Option<SharedWindowMetrics>,
}

impl Runtime {
    pub fn start() -> Result<Self, String> {
        let runtime = runtime::Builder::new_current_thread()
            .build()
            .map_err(|e| e.to_string())?;
        let local = LocalSet::new();
        let (pages, page_rx) = watch::channel(None);
        let (ticks, tick_rx) = watch::channel(0);
        let (reports_tx, reports) = mpsc::channel(128);
        let (input, input_rx) = mpsc::channel(INPUT_CAPACITY);
        let pasted = Rc::new(RefCell::new(VecDeque::new()));
        let (losses, loss_rx) = watch::channel(InputLoss::default());
        let initial = Rc::new(RefCell::new(InitialRender::default()));
        let exhausted = Rc::new(Cell::new(false));
        let task = local.spawn_local(worker(
            page_rx,
            tick_rx,
            reports_tx,
            input_rx,
            loss_rx,
            pasted.clone(),
            initial.clone(),
            exhausted.clone(),
        ));
        let mut service = Self {
            load_resources: false,
            local,
            runtime,
            task,
            pages,
            ticks,
            reports,
            input,
            pasted,
            losses,
            sequence: 0,
            revision: 0,
            tick: 0,
            initial,
            exhausted,
            window: None,
        };
        service.drive();
        Ok(service)
    }

    /// Store an inert page batch. Replacing a page also drops its VM immediately.
    /// Applies to subsequent pages. Production enables this; offline callers
    /// opt in only after installing their captured-resource ticket backend.
    pub fn enable_resource_loading(&mut self) {
        self.load_resources = true;
    }

    /// Store an inert page batch. Replacing a page also drops its VM immediately.
    pub fn replace_page(
        &mut self,
        generation: u64,
        url: impl Into<String>,
        sources: Vec<ScriptSource>,
    ) {
        self.store_page(generation, url.into(), sources, None);
    }

    /// A completed parse explicitly admits the finite initial inline script set.
    /// Layout receives a frozen clone; later script mutations stay in this tree.
    pub fn begin_page(
        &mut self,
        generation: u64,
        url: impl Into<String>,
        sources: Vec<ScriptSource>,
        document: rp5::Document,
    ) {
        self.store_page(
            generation,
            url.into(),
            sources,
            Some(Rc::new(RefCell::new(document))),
        );
    }

    fn store_page(
        &mut self,
        generation: u64,
        url: String,
        sources: Vec<ScriptSource>,
        document: Option<SharedDocument>,
    ) {
        self.revision = self.revision.wrapping_add(1).max(1);
        self.pasted.borrow_mut().clear();
        let automatic = document.is_some();
        self.exhausted.set(false);
        *self.initial.borrow_mut() = InitialRender {
            generation,
            pending: automatic,
            snapshot: None,
        };
        self.pages.send_replace(Some(Page {
            revision: self.revision,
            generation,
            url,
            sources: sources.into(),
            requested: automatic,
            active: automatic,
            document,
            current_script: Rc::new(Cell::new(None)),
            load_resources: self.load_resources,
            window: if automatic { self.window.clone() } else { None },
            initial_started: automatic.then(Instant::now),
        }));
        // The local task shares this thread. It processes the reset before any
        // other evaluation and cannot race the caller's navigation state.
        self.drive();
    }

    /// Called after native creation/resize has updated the very same content
    /// viewport used by layout. State reads do not execute guest callbacks.
    pub fn update_viewport(&mut self, inner_width: u32, inner_height: u32) {
        let metrics = WindowMetrics {
            inner_width,
            inner_height,
        };
        if let Some(window) = &self.window {
            window.set(metrics);
        } else {
            self.window = Some(Rc::new(Cell::new(metrics)));
        }
    }

    pub fn is_initializing(&self, generation: u64) -> bool {
        let initial = self.initial.borrow();
        initial.generation == generation && initial.pending
    }

    pub fn take_initial_document(&mut self, generation: u64) -> Option<rp5::Document> {
        let mut initial = self.initial.borrow_mut();
        if initial.generation != generation {
            return None;
        }
        initial.snapshot.take()
    }

    /// Explicit capability, deliberately not called by Hull's page-loading path.
    pub fn run(&mut self) -> Result<(), &'static str> {
        if self.exhausted.get() {
            return Err("page JavaScript execution budget exhausted");
        }
        let Some(mut page) = self.pages.borrow().clone() else {
            return Err("no page is loaded");
        };
        if page.requested {
            return Err("scripts already requested for this page");
        }
        page.requested = true;
        page.active = true;
        self.pages.send_replace(Some(page));
        Ok(())
    }

    /// Explicit console execution shares this page's VM without requesting its
    /// collected scripts. Navigation discards queued pastes along with the VM.
    pub fn execute_pasted(&mut self, source: String) -> Result<(), &'static str> {
        if self.exhausted.get() {
            return Err("page JavaScript execution budget exhausted");
        }
        if source.len() > 64 * 1024 {
            return Err("script exceeds 64 KiB");
        }
        let Some(mut page) = self.pages.borrow().clone() else {
            return Err("load a page before running a script");
        };
        let mut pasted = self.pasted.borrow_mut();
        if pasted.len() >= 8 {
            return Err("script queue is full");
        }
        pasted.push_back(PastedScript {
            revision: page.revision,
            generation: page.generation,
            source,
        });
        page.active = true;
        self.pages.send_replace(Some(page));
        Ok(())
    }

    /// Input is data only; it never creates a VM or authorizes page scripts.
    pub fn enqueue_input(&mut self, generation: u64, mut event: Value) -> EnqueueResult {
        if self.exhausted.get() {
            return EnqueueResult::Dormant;
        }
        let (revision, active) = {
            let page = self.pages.borrow();
            let Some(page) = page.as_ref() else {
                return EnqueueResult::Dormant;
            };
            if page.generation != generation {
                return EnqueueResult::Stale;
            }
            (page.revision, page.active)
        };
        if !active {
            return EnqueueResult::Dormant;
        }
        let Some(object) = event.as_object_mut() else {
            return EnqueueResult::Invalid;
        };
        let Some(kind) = object
            .get("kind")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 64 && *s != "*")
            .map(str::to_owned)
        else {
            return EnqueueResult::Invalid;
        };
        let window = object
            .get("window_id")
            .and_then(Value::as_u64)
            .is_some_and(|id| id <= u32::MAX as u64);
        let global_reset = kind == "input-reset"
            && object.get("window_id").is_some_and(Value::is_null)
            && object.get("reset_all").and_then(Value::as_bool) == Some(true)
            && object.get("scope").and_then(Value::as_str) == Some("all-windows");
        if !window && !global_reset {
            return EnqueueResult::Invalid;
        }
        self.sequence = self.sequence.wrapping_add(1).max(1);
        object.insert("version".into(), json!(1));
        object.insert("generation".into(), json!(generation.to_string()));
        object.insert("revision".into(), json!(revision.to_string()));
        object.insert("sequence".into(), json!(self.sequence.to_string()));
        let Ok(bytes) = serde_json::to_vec(&event) else {
            return EnqueueResult::Invalid;
        };
        if bytes.len() > INPUT_BYTES {
            return EnqueueResult::Invalid;
        }
        let packet = InputPacket {
            revision,
            generation,
            kind,
            bytes,
        };
        match self.input.try_send(packet) {
            Ok(()) => EnqueueResult::Accepted,
            Err(mpsc::error::TrySendError::Full(_)) => {
                let count = self.losses.borrow().count.saturating_add(1);
                self.losses.send_replace(InputLoss {
                    count,
                    sequence: self.sequence,
                });
                EnqueueResult::Overflow
            }
            Err(mpsc::error::TrySendError::Closed(_)) => EnqueueResult::Stale,
        }
    }

    pub fn poll(&mut self) -> Vec<Report> {
        self.drive();
        let mut reports = Vec::new();
        while let Ok(report) = self.reports.try_recv() {
            reports.push(report);
        }
        reports
    }

    fn drive(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.ticks.send_replace(self.tick);
        // An immediately-ready future would never poll the LocalSet. The tick
        // token, rather than yield_now alone, bounds the actor to one step.
        self.local
            .block_on(&self.runtime, async { task::yield_now().await });
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.task.abort();
        self.local
            .block_on(&self.runtime, async { task::yield_now().await });
    }
}

async fn worker(
    mut pages: watch::Receiver<Option<Page>>,
    mut ticks: watch::Receiver<u64>,
    reports: mpsc::Sender<Report>,
    mut inputs: mpsc::Receiver<InputPacket>,
    mut losses: watch::Receiver<InputLoss>,
    pasted: Rc<RefCell<VecDeque<PastedScript>>>,
    initial: Rc<RefCell<InitialRender>>,
    exhausted: Rc<Cell<bool>>,
) {
    let _ = reports.try_send(Report::Online);
    let mut page: Option<Page> = None;
    let mut engine: Option<Engine> = None;
    let mut resources: Option<Loader> = None;
    let mut started = false;
    let mut next = 0;
    let mut seen_loss = 0;
    let mut reset: Option<PendingReset> = None;
    while ticks.changed().await.is_ok() {
        if pages.has_changed().unwrap_or(false) {
            let incoming = pages.borrow_and_update().clone();
            if incoming.as_ref().map(|p| p.revision) != page.as_ref().map(|p| p.revision) {
                resources = None; // Cancels tickets before replacing the realm.
                engine = None; // Context, globals, pending promises and limits all go together.
                next = 0;
                started = false;
                reset = None;
                while inputs.try_recv().is_ok() {}
                seen_loss = losses.borrow_and_update().count;
                if let Some(p) = &incoming {
                    let _ = reports.try_send(Report::Page {
                        generation: p.generation,
                        sources: p.sources.len(),
                    });
                }
            }
            page = incoming;
        }
        let Some(page) = &page else {
            continue;
        };
        if losses.has_changed().unwrap_or(false) {
            let loss = *losses.borrow_and_update();
            let mut discarded = loss.count.saturating_sub(seen_loss);
            while inputs.try_recv().is_ok() {
                discarded = discarded.saturating_add(1);
            }
            seen_loss = loss.count;
            let pending = reset.get_or_insert(PendingReset {
                dropped: 0,
                sequence: loss.sequence,
            });
            pending.dropped = pending.dropped.saturating_add(discarded);
            pending.sequence = loss.sequence;
        }
        if !page.active {
            continue;
        }
        if !started {
            started = true;
            match Engine::new() {
                Ok(mut value) => {
                    value.set_execution_budget(PAGE_EXECUTION_BUDGET);
                    let initialized = value
                        .install_console()
                        .and_then(|_| value.install_clock())
                        .and_then(|_| {
                            value.eval(
                                include_bytes!("javascript_helpers.js"),
                                "hull:helpers",
                                false,
                            )
                        })
                        .and_then(|_| {
                            value.eval(
                                include_bytes!("javascript_events.js"),
                                "hull:input-bridge",
                                false,
                            )
                        })
                        .and_then(|_| value.bind_message_handler("__hullInputReceive"));
                    let initialized = initialized.and_then(|_| {
                        if let Some(document) = &page.document {
                            javascript_dom::install_for_page(
                                &mut value,
                                document.clone(),
                                page.window.clone(),
                                page.current_script.clone(),
                            )
                        } else {
                            Ok(())
                        }
                    });
                    match initialized {
                        Ok(()) => {
                            if page.load_resources {
                                let document = page.document.as_ref().map(|d| d.borrow());
                                match Loader::new(&mut value, &page.url, document.as_deref()) {
                                    Ok(loader) => resources = Some(loader),
                                    Err(error) => {
                                        let _ = reports.try_send(Report::Resource {
                                            generation: page.generation,
                                            url: page.url.clone(),
                                            status: "configuration-failed",
                                            detail: error,
                                        });
                                    }
                                }
                            }
                            engine = Some(value)
                        }
                        Err(error) => {
                            let _ = reports.try_send(Report::JobError {
                                generation: page.generation,
                                error,
                            });
                            finish_initial(page, &initial, &reports, true, next);
                            exhausted.set(true);
                            continue;
                        }
                    }
                }
                Err(error) => {
                    let _ = reports.try_send(Report::JobError {
                        generation: page.generation,
                        error,
                    });
                    finish_initial(page, &initial, &reports, true, next);
                    exhausted.set(true);
                    continue;
                }
            }
        }
        let Some(engine) = &mut engine else {
            continue;
        };
        if engine.execution_budget_exhausted() || exhausted.get() {
            if let Some(loader) = &mut resources {
                loader.close();
            }
            if !exhausted.replace(true) {
                let _ = reports.try_send(Report::BudgetExhausted {
                    generation: page.generation,
                });
            }
            finish_initial(page, &initial, &reports, true, next);
            while inputs.try_recv().is_ok() {}
            pasted.borrow_mut().clear();
            continue;
        }
        if let Some(loader) = &mut resources {
            loader.poll(engine);
            for event in loader.events.drain(..) {
                let _ = reports.try_send(Report::Resource {
                    generation: page.generation,
                    url: event.url,
                    status: event.status,
                    detail: event.detail,
                });
            }
        }
        if initial.borrow().pending
            && page.initial_started.is_some_and(|started| {
                let waited = resources.as_ref().map_or(Duration::ZERO, Loader::waited);
                started.elapsed().saturating_sub(waited) >= INITIAL_RENDER_WAIT
                    || started.elapsed() >= INITIAL_RENDER_WAIT + module_loader::MAX_WAIT
            })
        {
            finish_initial(page, &initial, &reports, true, next);
            next = page.sources.len();
        }
        if engine.has_pending_jobs() {
            let until = Instant::now() + Duration::from_millis(25);
            for _ in 0..16 {
                if Instant::now() >= until {
                    break;
                }
                match engine.execute_pending_job() {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        let _ = reports.try_send(Report::JobError {
                            generation: page.generation,
                            error,
                        });
                        break;
                    }
                }
            }
            flush_console(engine, page.generation, &reports);
            continue;
        }
        let source = if page.requested {
            page.sources.get(next)
        } else {
            None
        };
        let Some(source) = source else {
            if resources.as_ref().is_some_and(Loader::pending) {
                continue;
            }
            if resources.is_some() {
                for error in engine.take_unhandled_rejections() {
                    let _ = reports.try_send(Report::JobError {
                        generation: page.generation,
                        error,
                    });
                }
            }
            if initial.borrow().pending {
                finish_initial(page, &initial, &reports, false, next);
                continue;
            }
            // Pasted work is explicit and runs one source per turn after any
            // requested page scripts and their Promise checkpoints.
            let mut selected = None;
            for _ in 0..8 {
                let Some(script) = pasted.borrow_mut().pop_front() else {
                    break;
                };
                if script.revision == page.revision && script.generation == page.generation {
                    selected = Some(script);
                    break;
                }
            }
            if let Some(script) = selected {
                let result = engine.eval(script.source.as_bytes(), "hull:console-paste", false);
                flush_console(engine, page.generation, &reports);
                let _ = reports.try_send(Report::Pasted {
                    generation: page.generation,
                    result,
                });
                continue;
            }
            // A bounded batch can keep up with native motion streams. Promise
            // jobs run between messages before another callback receives data.
            let until = Instant::now() + Duration::from_millis(25);
            for _ in 0..32 {
                if Instant::now() >= until {
                    break;
                }
                let event = if let Some(pending) = reset.take() {
                    let value = json!({"version":1,"kind":"input-reset","reason":"bridge-overflow","window_id":null,"scope":"all-windows","reset_all":true,"dropped":pending.dropped.to_string(),"generation":page.generation.to_string(),"revision":page.revision.to_string(),"sequence":pending.sequence.to_string()});
                    Some((
                        "input-reset".to_owned(),
                        serde_json::to_vec(&value).expect("bounded reset serializes"),
                    ))
                } else {
                    let mut selected = None;
                    for _ in 0..INPUT_CAPACITY {
                        let Ok(packet) = inputs.try_recv() else {
                            break;
                        };
                        if packet.revision == page.revision && packet.generation == page.generation
                        {
                            selected = Some((packet.kind, packet.bytes));
                            break;
                        }
                    }
                    selected
                };
                let Some((kind, bytes)) = event else {
                    break;
                };
                let result = engine.dispatch_json(&bytes);
                let _ = reports.try_send(Report::Input {
                    generation: page.generation,
                    kind,
                    result,
                });
                for _ in 0..16 {
                    if Instant::now() >= until {
                        break;
                    }
                    match engine.execute_pending_job() {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(error) => {
                            let _ = reports.try_send(Report::JobError {
                                generation: page.generation,
                                error,
                            });
                            break;
                        }
                    }
                }
                if engine.has_pending_jobs() {
                    break;
                }
            }
            flush_console(engine, page.generation, &reports);
            continue;
        };
        next += 1;
        let kind = source.kind.split(';').next().unwrap_or("").trim();
        let module = kind.eq_ignore_ascii_case("module");
        let javascript = module
            || kind.is_empty()
            || [
                "text/javascript",
                "application/javascript",
                "text/ecmascript",
                "application/ecmascript",
            ]
            .iter()
            .any(|mime| kind.eq_ignore_ascii_case(mime));
        if !javascript {
            let _ = reports.try_send(Report::Skipped {
                generation: page.generation,
                origin: source.origin.clone(),
                reason: "non-JavaScript script type",
            });
            continue;
        }
        let node = source.nodes.first().copied().unwrap_or(0);
        let external = source.bytes.is_none()
            || page.document.as_ref().is_some_and(|doc| {
                doc.borrow().element(node).is_some_and(|(_, _, attrs)| {
                    attrs
                        .iter()
                        .any(|a| matches!(a.name.as_str(), "src" | "href"))
                })
            });
        let mut origin = if external {
            source.origin.clone()
        } else {
            format!("{}#script-{node}", source.origin)
        };
        let downloaded;
        let mut prepared_module = false;
        let bytes = if let Some(bytes) = &source.bytes {
            bytes.as_slice()
        } else if let Some(loader) = &mut resources {
            let state = if module {
                match loader.module_source(engine, &source.origin) {
                    Prepared::Ready((url, bytes)) => {
                        origin = url;
                        prepared_module = true;
                        Prepared::Ready(bytes)
                    }
                    Prepared::Pending => Prepared::Pending,
                    Prepared::Failed(e) => Prepared::Failed(e),
                }
            } else {
                loader.source(&source.origin)
            };
            match state {
                Prepared::Pending => {
                    next -= 1;
                    continue;
                }
                Prepared::Ready(bytes) => {
                    downloaded = bytes;
                    downloaded.as_ref()
                }
                Prepared::Failed(error) => {
                    let _ = reports.try_send(Report::Evaluated {
                        generation: page.generation,
                        origin,
                        node,
                        result: Err(error),
                    });
                    continue;
                }
            }
        } else {
            let _ = reports.try_send(Report::Skipped {
                generation: page.generation,
                origin: source.origin.clone(),
                reason: "external source is not loaded",
            });
            continue;
        };
        if module && !prepared_module {
            if let Some(loader) = &mut resources {
                match loader.inline(engine, &origin, bytes) {
                    Prepared::Pending => {
                        next -= 1;
                        continue;
                    }
                    Prepared::Failed(error) => {
                        let _ = reports.try_send(Report::Evaluated {
                            generation: page.generation,
                            origin,
                            node,
                            result: Err(error),
                        });
                        continue;
                    }
                    Prepared::Ready(()) => {}
                }
            }
        }
        let current_script = if module {
            None
        } else {
            page.document.as_ref().and_then(|doc| {
                let doc = doc.borrow();
                doc.element(node).and_then(|(name, namespace, _)| {
                    (name == "script"
                        && matches!(namespace, rp5::Namespace::Html | rp5::Namespace::Svg))
                    .then_some(node)
                })
            })
        };
        page.current_script.set(current_script);
        let result = engine.eval(bytes, &origin, module);
        // Clear even on syntax errors or interruption, before any queued jobs,
        // console delivery, pasted code or input callbacks are admitted.
        page.current_script.set(None);
        flush_console(engine, page.generation, &reports);
        let _ = reports.try_send(Report::Evaluated {
            generation: page.generation,
            origin,
            node,
            result,
        });
    }
}

fn finish_initial(
    page: &Page,
    initial: &RefCell<InitialRender>,
    reports: &mpsc::Sender<Report>,
    capped: bool,
    next: usize,
) {
    let mut initial = initial.borrow_mut();
    if !initial.pending || initial.generation != page.generation {
        return;
    }
    initial.snapshot = page
        .document
        .as_ref()
        .map(|document| document.borrow().clone());
    initial.pending = false;
    let _ = reports.try_send(Report::Initialized {
        generation: page.generation,
        capped,
        remaining_sources: page.sources.len().saturating_sub(next),
    });
}

fn flush_console(engine: &mut Engine, generation: u64, reports: &mpsc::Sender<Report>) {
    let batch = engine.take_console();
    if !batch.messages.is_empty() || batch.dropped != 0 {
        let _ = reports.try_send(Report::Console { generation, batch });
    }
}
