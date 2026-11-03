//! Page JavaScript owned and serialized by one native service lane.
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use hull_quickjs::{ConsoleBatch, Engine};
use serde_json::{Value, json};
use trueos::tokio::{
    runtime,
    sync::{Notify, mpsc, watch},
    task::{self, LocalSet},
};

use crate::javascript_dom::{
    self, SharedCurrentScript, SharedDocument, SharedWindowMetrics, WindowMetrics,
};
use crate::scripts::ScriptSource;
#[path = "module_loader.rs"]
pub(super) mod module_loader;
use crate::javascript_dom::timers;
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

// Only owned data crosses the native lane boundary. Rc DOM and QuickJS state
// are constructed and destroyed exclusively inside that lane.
#[derive(Clone)]
struct Page {
    revision: u64,
    generation: u64,
    url: String,
    sources: Arc<[ScriptSource]>,
    requested: bool,
    active: bool,
    document: Option<Arc<rp5::Document>>,
    load_resources: bool,
    initial_started: Option<Instant>,
}
#[derive(Default)]
struct Published {
    revision: u64,
    generation: u64,
    cookies_version: Option<u64>,
    cookies: String,
    storage: String,
    media: Option<(u64, javascript_dom::media::Request)>,
    receipt: Option<(u64, u32, i32)>,
}

#[derive(Clone)]
struct LocalPage {
    revision: u64,
    generation: u64,
    url: String,
    sources: Arc<[ScriptSource]>,
    requested: bool,
    active: bool,
    document: Option<SharedDocument>,
    current_script: SharedCurrentScript,
    cookies: javascript_dom::cookies::SharedCookies,
    media: javascript_dom::media::SharedMedia,
    load_resources: bool,
    window: Option<SharedWindowMetrics>,
    initial_started: Option<Instant>,
}

#[derive(Default)]
struct InitialRender {
    revision: u64,
    generation: u64,
    pending: bool,
    snapshot: Option<rp5::Document>,
}

pub(super) struct Runtime {
    load_resources: bool,
    _task: trueos::worker::JoinHandle<()>,
    wake: Arc<Notify>,
    stopped: Arc<AtomicBool>,
    published: Arc<Mutex<Published>>,
    pages: watch::Sender<Option<Page>>,
    reports: mpsc::Receiver<Report>,
    input: mpsc::Sender<InputPacket>,
    pasted: Arc<Mutex<VecDeque<PastedScript>>>,
    losses: watch::Sender<InputLoss>,
    sequence: u64,
    revision: u64,
    initial: Arc<Mutex<InitialRender>>,
    exhausted: Arc<AtomicU64>,
    window: Arc<Mutex<Option<WindowMetrics>>>,
}

impl Runtime {
    pub fn start() -> Result<Self, String> {
        let (pages, page_rx) = watch::channel(None);
        let (reports_tx, reports) = mpsc::channel(128);
        let (input, input_rx) = mpsc::channel(INPUT_CAPACITY);
        let pasted = Arc::new(Mutex::new(VecDeque::new()));
        let (losses, loss_rx) = watch::channel(InputLoss::default());
        let initial = Arc::new(Mutex::new(InitialRender::default()));
        let exhausted = Arc::new(AtomicU64::new(0));
        let wake = Arc::new(Notify::new());
        let stopped = Arc::new(AtomicBool::new(false));
        let published = Arc::new(Mutex::new(Published::default()));
        let window = Arc::new(Mutex::new(None));
        let task = {
            let pasted = pasted.clone();
            let initial = initial.clone();
            let exhausted = exhausted.clone();
            let wake = wake.clone();
            let stopped = stopped.clone();
            let published = published.clone();
            let window = window.clone();
            trueos::worker::spawn(move || {
                let runtime = runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .expect("JavaScript lane runtime");
                let local = LocalSet::new();
                local.block_on(
                    &runtime,
                    worker(
                        page_rx, reports_tx, input_rx, loss_rx, pasted, initial, exhausted, wake,
                        stopped, published, window,
                    ),
                );
            })
            .map_err(|error| error.to_string())?
        };
        Ok(Self {
            load_resources: false,
            _task: task,
            pages,
            reports,
            input,
            pasted,
            losses,
            sequence: 0,
            revision: 0,
            initial,
            exhausted,
            wake,
            stopped,
            published,
            window,
        })
    }

    /// Applies to subsequent pages. Production enables this; offline callers
    /// opt in only after installing their captured-resource ticket backend.
    pub fn enable_resource_loading(&mut self) {
        self.load_resources = true;
    }

    /// Store an inert page batch. The lane drops the previous realm before
    /// admitting any work for the new revision.
    pub fn replace_page(
        &mut self,
        generation: u64,
        url: impl Into<String>,
        sources: Vec<ScriptSource>,
    ) {
        self.store_page(generation, url.into(), sources, None);
    }

    /// A completed parse admits the finite page script set. Opted-in source
    /// loading prepares external bodies and module graphs cooperatively.
    /// Layout receives a frozen clone; later script mutations stay in this tree.
    pub fn begin_page(
        &mut self,
        generation: u64,
        url: impl Into<String>,
        sources: Vec<ScriptSource>,
        document: rp5::Document,
    ) {
        self.store_page(generation, url.into(), sources, Some(document));
    }

    fn store_page(
        &mut self,
        generation: u64,
        url: String,
        sources: Vec<ScriptSource>,
        document: Option<rp5::Document>,
    ) {
        self.revision = self.revision.wrapping_add(1).max(1);
        self.pasted.lock().unwrap().clear();
        let automatic = document.is_some();
        self.exhausted.store(0, Ordering::Release);
        *self.initial.lock().unwrap() = InitialRender {
            revision: self.revision,
            generation,
            pending: automatic,
            snapshot: None,
        };
        *self.published.lock().unwrap() = Published {
            revision: self.revision,
            generation,
            ..Default::default()
        };
        self.pages.send_replace(Some(Page {
            revision: self.revision,
            generation,
            url,
            sources: sources.into(),
            requested: automatic,
            active: automatic,
            document: document.map(Arc::new),
            load_resources: self.load_resources,
            initial_started: automatic.then(Instant::now),
        }));
        self.wake.notify_one();
    }

    /// Called after native creation/resize has updated the very same content
    /// viewport used by layout. State reads do not execute guest callbacks.
    pub fn update_viewport(&mut self, inner_width: u32, inner_height: u32) {
        let metrics = WindowMetrics {
            inner_width,
            inner_height,
        };
        *self.window.lock().unwrap() = Some(metrics);
        self.wake.notify_one();
    }

    pub fn take_video_request(&self) -> Option<(u64, javascript_dom::media::Request)> {
        self.published.lock().unwrap().media.take()
    }
    pub fn video_status(&self, generation: u64, id: u32, status: i32) {
        let mut state = self.published.lock().unwrap();
        if state.revision == 0 || state.generation != generation {
            return;
        }
        state.receipt = Some((generation, id, status));
        drop(state);
        self.wake.notify_one();
    }
    pub fn cookie_snapshot(&self) -> String {
        self.published.lock().unwrap().cookies.clone()
    }
    pub fn storage_snapshot(&self) -> String {
        let state = self.published.lock().unwrap();
        if state.storage.is_empty() {
            "[]".into()
        } else {
            state.storage.clone()
        }
    }

    pub fn is_initializing(&self, generation: u64) -> bool {
        let initial = self.initial.lock().unwrap();
        initial.generation == generation && initial.pending
    }

    pub fn take_initial_document(&mut self, generation: u64) -> Option<rp5::Document> {
        let mut initial = self.initial.lock().unwrap();
        if initial.generation != generation {
            return None;
        }
        initial.snapshot.take()
    }

    /// Explicit capability, deliberately not called by Hull's page-loading path.
    pub fn run(&mut self) -> Result<(), &'static str> {
        if self.revision != 0 && self.exhausted.load(Ordering::Acquire) == self.revision {
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
        self.wake.notify_one();
        Ok(())
    }

    /// Explicit console execution shares this page's VM without requesting its
    /// collected scripts. Navigation discards queued pastes along with the VM.
    pub fn execute_pasted(&mut self, source: String) -> Result<(), &'static str> {
        if self.revision != 0 && self.exhausted.load(Ordering::Acquire) == self.revision {
            return Err("page JavaScript execution budget exhausted");
        }
        if source.len() > 64 * 1024 {
            return Err("script exceeds 64 KiB");
        }
        let Some(mut page) = self.pages.borrow().clone() else {
            return Err("load a page before running a script");
        };
        let mut pasted = self.pasted.lock().unwrap();
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
        self.wake.notify_one();
        Ok(())
    }

    /// Input is data only; it never creates a VM or authorizes page scripts.
    pub fn enqueue_input(&mut self, generation: u64, mut event: Value) -> EnqueueResult {
        if self.revision != 0 && self.exhausted.load(Ordering::Acquire) == self.revision {
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
            Ok(()) => {
                self.wake.notify_one();
                EnqueueResult::Accepted
            }
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
        let mut reports = Vec::new();
        while let Ok(report) = self.reports.try_recv() {
            reports.push(report);
        }
        reports
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.wake.notify_one();
    }
}

async fn worker(
    mut pages: watch::Receiver<Option<Page>>,
    reports: mpsc::Sender<Report>,
    mut inputs: mpsc::Receiver<InputPacket>,
    mut losses: watch::Receiver<InputLoss>,
    pasted: Arc<Mutex<VecDeque<PastedScript>>>,
    initial: Arc<Mutex<InitialRender>>,
    exhausted: Arc<AtomicU64>,
    wake: Arc<Notify>,
    stopped: Arc<AtomicBool>,
    published: Arc<Mutex<Published>>,
    window: Arc<Mutex<Option<WindowMetrics>>>,
) {
    let _ = reports.try_send(Report::Online);
    let mut page: Option<LocalPage> = None;
    let mut engine: Option<Engine> = None;
    let mut resources: Option<Loader> = None;
    let mut timers: Option<timers::SharedTimers> = None;
    let mut started = false;
    let mut dom_ready_sent = false;
    let mut next = 0;
    let mut seen_loss = 0;
    let mut reset: Option<PendingReset> = None;
    let mut deferred_input: Option<InputPacket> = None;
    loop {
        if stopped.load(Ordering::Acquire) || trueos::worker::cancellation_requested() {
            break;
        }
        let notified = wake.notified();
        if let Some(page) = &page {
            if let Some(metrics) = *window.lock().unwrap() {
                if let Some(window) = &page.window {
                    window.set(metrics);
                }
            }
            publish(page, &published);
        }
        let runnable = page.as_ref().is_some_and(|page| page.active)
            && page
                .as_ref()
                .is_none_or(|page| exhausted.load(Ordering::Acquire) != page.revision)
            && (engine
                .as_ref()
                .is_none_or(|engine| engine.has_pending_jobs())
                || page
                    .as_ref()
                    .is_some_and(|page| page.requested && next < page.sources.len())
                || page
                    .as_ref()
                    .is_some_and(|page| initial_pending(&initial, page.revision))
                || !pasted.lock().unwrap().is_empty()
                || deferred_input.is_some()
                || !inputs.is_empty()
                || reset.is_some()
                || page
                    .as_ref()
                    .is_some_and(|page| page.media.borrow().receipt.is_some()));
        let mut delay = if runnable {
            Duration::ZERO
        } else {
            Duration::from_millis(50)
        };
        if resources.as_ref().is_some_and(Loader::pending) {
            delay = Duration::from_millis(2);
        }
        if page
            .as_ref()
            .is_none_or(|page| exhausted.load(Ordering::Acquire) != page.revision)
        {
            if let Some(deadline) = timers
                .as_ref()
                .and_then(|timers| timers.borrow().next_deadline())
                .filter(|_| !resources.as_ref().is_some_and(Loader::pending))
            {
                delay = delay.min(deadline);
            }
        }
        if !pages.has_changed().unwrap_or(false) {
            if delay.is_zero() {
                task::yield_now().await;
            } else {
                let sleep = trueos::tokio::time::sleep(delay);
                trueos::tokio::pin!(sleep);
                trueos::tokio::pin!(notified);
                std::future::poll_fn(|cx| {
                    use std::future::Future;
                    if notified.as_mut().poll(cx).is_ready() {
                        return std::task::Poll::Ready(());
                    }
                    sleep.as_mut().poll(cx)
                })
                .await;
            }
        }
        if stopped.load(Ordering::Acquire) || trueos::worker::cancellation_requested() {
            break;
        }
        if pages.has_changed().unwrap_or(false) {
            let incoming = pages.borrow_and_update().clone();
            if incoming.as_ref().map(|p| p.revision) != page.as_ref().map(|p| p.revision) {
                resources = None; // Cancels tickets before replacing the realm.
                engine = None; // Context, globals, pending promises and limits all go together.
                timers = None;
                next = 0;
                started = false;
                dom_ready_sent = false;
                reset = None;
                seen_loss = losses.borrow_and_update().count;
                if let Some(p) = &incoming {
                    let _ = reports.try_send(Report::Page {
                        generation: p.generation,
                        sources: p.sources.len(),
                    });
                }
            }
            if incoming.as_ref().map(|p| p.revision) != page.as_ref().map(|p| p.revision) {
                page = incoming.map(|p| LocalPage {
                    revision: p.revision,
                    generation: p.generation,
                    url: p.url,
                    sources: p.sources,
                    requested: p.requested,
                    active: p.active,
                    document: p.document.map(|d| Rc::new(RefCell::new((*d).clone()))),
                    current_script: Rc::new(Cell::new(None)),
                    cookies: Rc::new(RefCell::new(Default::default())),
                    media: Rc::new(RefCell::new(Default::default())),
                    window: window
                        .lock()
                        .unwrap()
                        .map(|metrics| Rc::new(Cell::new(metrics))),
                    load_resources: p.load_resources,
                    initial_started: p.initial_started,
                });
            } else if let (Some(local), Some(incoming)) = (&mut page, incoming) {
                local.requested = incoming.requested;
                local.active = incoming.active;
            }
        }
        let Some(page) = &page else {
            continue;
        };
        // Commands may have arrived while the lane was parked. Refresh before
        // dispatching callbacks from that wake, including pasted scripts.
        if let (Some(metrics), Some(window)) = (*window.lock().unwrap(), &page.window) {
            window.set(metrics);
        }
        if losses.has_changed().unwrap_or(false) {
            let loss = *losses.borrow_and_update();
            let mut discarded = loss.count.saturating_sub(seen_loss);
            while let Some(packet) = deferred_input.take().or_else(|| inputs.try_recv().ok()) {
                if packet.revision > page.revision {
                    deferred_input = Some(packet);
                    break;
                }
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
                        });
                    let initialized = initialized.and_then(|_| {
                        timers = Some(Rc::new(RefCell::new(timers::Timers::default())));
                        Ok(())
                    });
                    let initialized = initialized.and_then(|_| {
                        if let Some(document) = &page.document {
                            javascript_dom::install_for_page_with_cookies(
                                &mut value,
                                document.clone(),
                                page.window.clone(),
                                page.current_script.clone(),
                                &page.url,
                                page.cookies.clone(),
                                timers.clone(),
                                Some(page.media.clone()),
                            )
                        } else {
                            javascript_dom::install_url_with_timers(&mut value, timers.clone())
                                .and_then(|_| {
                                    value.eval(
                                        include_bytes!("javascript_events.js"),
                                        "hull:input-bridge",
                                        false,
                                    )
                                })
                                .and_then(|_| value.bind_message_handler("__hullInputReceive"))
                        }
                    });
                    match initialized {
                        Ok(()) => {
                            if page.load_resources {
                                let document = page.document.as_ref().map(|d| d.borrow());
                                match Loader::new(&mut value, &page.url, document.as_deref()) {
                                    Ok(mut loader) => {
                                        for source in page
                                            .sources
                                            .iter()
                                            .filter(|s| s.bytes.is_none())
                                            .take(module_loader::MAX_FILES)
                                        {
                                            let kind =
                                                source.kind.split(';').next().unwrap_or("").trim();
                                            if kind.eq_ignore_ascii_case("module")
                                                || kind.is_empty()
                                                || [
                                                    "text/javascript",
                                                    "application/javascript",
                                                    "text/ecmascript",
                                                    "application/ecmascript",
                                                ]
                                                .iter()
                                                .any(|mime| kind.eq_ignore_ascii_case(mime))
                                            {
                                                loader.prefetch(
                                                    &source.origin,
                                                    kind.eq_ignore_ascii_case("module"),
                                                );
                                            }
                                        }
                                        resources = Some(loader)
                                    }
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
                            finish_initial(page, &initial, &published, &reports, true, next);
                            exhausted.store(page.revision, Ordering::Release);
                            continue;
                        }
                    }
                }
                Err(error) => {
                    let _ = reports.try_send(Report::JobError {
                        generation: page.generation,
                        error,
                    });
                    finish_initial(page, &initial, &published, &reports, true, next);
                    exhausted.store(page.revision, Ordering::Release);
                    continue;
                }
            }
        }
        let Some(engine) = &mut engine else {
            continue;
        };
        if engine.execution_budget_exhausted()
            || (exhausted.load(Ordering::Acquire) == page.revision)
        {
            if let Some(loader) = &mut resources {
                loader.close();
            }
            if exhausted.swap(page.revision, Ordering::AcqRel) != page.revision {
                let _ = reports.try_send(Report::BudgetExhausted {
                    generation: page.generation,
                });
            }
            finish_initial(page, &initial, &published, &reports, true, next);
            while let Some(packet) = deferred_input.take().or_else(|| inputs.try_recv().ok()) {
                if packet.revision > page.revision {
                    deferred_input = Some(packet);
                    break;
                }
            }
            pasted
                .lock()
                .unwrap()
                .retain(|script| script.revision != page.revision);
            continue;
        }
        let receipt = page.media.borrow_mut().receipt.take();
        if let Some((id, status)) = receipt {
            let result = engine.dispatch_json(
                json!({"kind":"__hull_media_receipt","id":id,"status":status})
                    .to_string()
                    .as_bytes(),
            );
            if let Err(error) = result {
                let _ = reports.try_send(Report::JobError {
                    generation: page.generation,
                    error,
                });
            }
            publish(page, &published);
            flush_console(engine, page.generation, &reports);
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
        if initial_pending(&initial, page.revision)
            && page.initial_started.is_some_and(|started| {
                let waited = resources.as_ref().map_or(Duration::ZERO, Loader::waited);
                started.elapsed().saturating_sub(waited) >= INITIAL_RENDER_WAIT
                    || started.elapsed() >= INITIAL_RENDER_WAIT + module_loader::MAX_WAIT
            })
        {
            finish_initial(page, &initial, &published, &reports, true, next);
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
            publish(page, &published);
            flush_console(engine, page.generation, &reports);
            continue;
        }
        let source = if page.requested {
            page.sources.get(next)
        } else {
            None
        };
        let Some(source) = source else {
            let due = timers
                .as_ref()
                .and_then(|timers| timers.borrow_mut().take_due());
            if let Some(id) = due {
                let result = engine.dispatch_json(
                    json!({"kind":"__hull_timer","id":id})
                        .to_string()
                        .as_bytes(),
                );
                if let Err(error) = result {
                    let _ = reports.try_send(Report::JobError {
                        generation: page.generation,
                        error,
                    });
                }
                publish(page, &published);
                flush_console(engine, page.generation, &reports);
                continue;
            }
            if resources.as_ref().is_some_and(Loader::pending) {
                continue;
            }
            if !dom_ready_sent && page.document.is_some() {
                dom_ready_sent = true;
                let result = engine.dispatch_json(br#"{"kind":"__hull_dom_ready"}"#);
                if let Err(error) = result {
                    let _ = reports.try_send(Report::JobError {
                        generation: page.generation,
                        error,
                    });
                }
                publish(page, &published);
                flush_console(engine, page.generation, &reports);
                // Ready callbacks and their Promise jobs run before capture.
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
            if initial_pending(&initial, page.revision) {
                finish_initial(page, &initial, &published, &reports, false, next);
                continue;
            }
            // Pasted work is explicit and runs one source per turn after any
            // requested page scripts and their Promise checkpoints.
            let mut selected = None;
            for _ in 0..8 {
                let script = {
                    let mut queue = pasted.lock().unwrap();
                    if queue
                        .front()
                        .is_some_and(|script| script.revision > page.revision)
                    {
                        break;
                    }
                    queue.pop_front()
                };
                let Some(script) = script else {
                    break;
                };
                if script.revision == page.revision && script.generation == page.generation {
                    selected = Some(script);
                    break;
                }
            }
            if let Some(script) = selected {
                let result = engine.eval(script.source.as_bytes(), "hull:console-paste", false);
                publish(page, &published);
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
                        let Some(packet) = deferred_input.take().or_else(|| inputs.try_recv().ok())
                        else {
                            break;
                        };
                        if packet.revision > page.revision {
                            deferred_input = Some(packet);
                            break;
                        }
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
                publish(page, &published);
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
            publish(page, &published);
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
        let result = if module && external && resources.is_some() {
            engine
                .add_module(&origin, bytes)
                .and_then(|_| engine.load_module(&origin))
        } else {
            engine.eval(bytes, &origin, module)
        };
        // Clear even on syntax errors or interruption, before any queued jobs,
        // console delivery, pasted code or input callbacks are admitted.
        page.current_script.set(None);
        publish(page, &published);
        flush_console(engine, page.generation, &reports);
        let _ = reports.try_send(Report::Evaluated {
            generation: page.generation,
            origin,
            node,
            result,
        });
    }
}

fn initial_pending(initial: &Mutex<InitialRender>, revision: u64) -> bool {
    let initial = initial.lock().unwrap();
    initial.revision == revision && initial.pending
}

fn publish(page: &LocalPage, published: &Mutex<Published>) {
    let mut state = published.lock().unwrap();
    if state.revision != page.revision {
        return;
    }
    let cookies = page.cookies.borrow();
    if state.cookies_version != Some(cookies.version()) {
        state.cookies = cookies.get();
        state.storage = cookies.snapshot();
        state.cookies_version = Some(cookies.version());
    }
    drop(cookies);
    if let Some(request) = page.media.borrow_mut().pending.take() {
        state.media = Some((page.generation, request));
    }
    if let Some((generation, id, status)) = state.receipt.take() {
        if generation == page.generation {
            page.media.borrow_mut().receipt = Some((id, status));
        }
    }
}

fn finish_initial(
    page: &LocalPage,
    initial: &Mutex<InitialRender>,
    published: &Mutex<Published>,
    reports: &mpsc::Sender<Report>,
    capped: bool,
    next: usize,
) {
    let mut initial = initial.lock().unwrap();
    if !initial.pending || initial.revision != page.revision {
        return;
    }
    publish(page, published);
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

#[cfg(test)]
mod native_lane_tests {
    use super::*;

    #[test]
    fn initialization_and_timers_progress_without_ui_polling() {
        let mut runtime = Runtime::start().unwrap();
        let document = rp5::parse(
            r#"<p>before</p><script>
            document.querySelector('p').textContent='initialized';
            setTimeout(()=>{document.cookie='worker=awake';},20);
            hullInput.addEventListener('focus',()=>{document.cookie='input=awake';});
        </script>"#,
        );
        let mut scripts = crate::scripts::Scripts::default();
        scripts.extract(&document, "https://fixture.test/native");
        runtime.begin_page(
            900,
            "https://fixture.test/native",
            scripts.sources,
            document,
        );
        let until = Instant::now() + Duration::from_secs(2);
        // These snapshot reads neither drive the worker nor notify it.
        while runtime.is_initializing(900) || runtime.cookie_snapshot() != "worker=awake" {
            assert!(
                Instant::now() < until,
                "native lane did not wake independently"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            runtime.enqueue_input(900, json!({"kind":"focus", "window_id":1})),
            EnqueueResult::Accepted
        );
        while !runtime.cookie_snapshot().contains("input=awake") {
            assert!(Instant::now() < until, "input did not wake native lane");
            std::thread::sleep(Duration::from_millis(1));
        }
        runtime
            .execute_pasted("document.cookie='paste=awake'".into())
            .unwrap();
        while !runtime.cookie_snapshot().contains("paste=awake") {
            assert!(Instant::now() < until, "paste did not wake native lane");
            std::thread::sleep(Duration::from_millis(1));
        }
        let document = runtime.take_initial_document(900).unwrap();
        let p = document
            .nodes
            .iter()
            .enumerate()
            .find(|(id, _)| document.name(*id) == "p")
            .unwrap()
            .0;
        assert_eq!(document.text_content(p), "initialized");
    }

    #[test]
    fn idle_lane_completes_after_shutdown_signal() {
        let mut runtime = Runtime::start().unwrap();
        runtime.stopped.store(true, Ordering::Release);
        runtime.wake.notify_one();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = runtime._task.try_take() {
                result.unwrap();
                break;
            }
            assert!(
                Instant::now() < until,
                "native lane did not release its closure"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn dropping_runtime_requests_shutdown_without_waiting_for_guest_code() {
        let mut runtime = Runtime::start().unwrap();
        runtime.replace_page(901, "https://fixture.test/native", Vec::new());
        runtime.execute_pasted("for(;;){}".into()).unwrap();
        let started = Instant::now();
        drop(runtime);
        assert!(started.elapsed() < Duration::from_millis(100));
    }
}
