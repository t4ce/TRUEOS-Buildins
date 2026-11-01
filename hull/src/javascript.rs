//! One idle Tokio local task. Page arrival never authorizes execution.
use std::{
    cell::RefCell,
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

use crate::scripts::ScriptSource;

#[derive(Debug)]
pub(super) enum Report {
    Online,
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
}

pub(super) struct Runtime {
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
        let task = local.spawn_local(worker(
            page_rx,
            tick_rx,
            reports_tx,
            input_rx,
            loss_rx,
            pasted.clone(),
        ));
        let mut service = Self {
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
        };
        service.drive();
        Ok(service)
    }

    /// Store an inert page batch. Replacing a page also drops its VM immediately.
    pub fn replace_page(
        &mut self,
        generation: u64,
        url: impl Into<String>,
        sources: Vec<ScriptSource>,
    ) {
        self.revision = self.revision.wrapping_add(1).max(1);
        self.pasted.borrow_mut().clear();
        self.pages.send_replace(Some(Page {
            revision: self.revision,
            generation,
            url: url.into(),
            sources: sources.into(),
            requested: false,
            active: false,
        }));
        // The local task shares this thread. It processes the reset before any
        // other evaluation and cannot race the caller's navigation state.
        self.drive();
    }

    /// Explicit capability, deliberately not called by Hull's page-loading path.
    pub fn run(&mut self) -> Result<(), &'static str> {
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
) {
    let _ = reports.try_send(Report::Online);
    let mut page: Option<Page> = None;
    let mut engine: Option<Engine> = None;
    let mut started = false;
    let mut next = 0;
    let mut seen_loss = 0;
    let mut reset: Option<PendingReset> = None;
    while ticks.changed().await.is_ok() {
        if pages.has_changed().unwrap_or(false) {
            let incoming = pages.borrow_and_update().clone();
            if incoming.as_ref().map(|p| p.revision) != page.as_ref().map(|p| p.revision) {
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
                    match initialized {
                        Ok(()) => engine = Some(value),
                        Err(error) => {
                            let _ = reports.try_send(Report::JobError {
                                generation: page.generation,
                                error,
                            });
                            continue;
                        }
                    }
                }
                Err(error) => {
                    let _ = reports.try_send(Report::JobError {
                        generation: page.generation,
                        error,
                    });
                    continue;
                }
            }
        }
        let Some(engine) = &mut engine else {
            continue;
        };
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
        let Some(bytes) = &source.bytes else {
            let _ = reports.try_send(Report::Skipped {
                generation: page.generation,
                origin: source.origin.clone(),
                reason: "external source is not loaded",
            });
            continue;
        };
        let node = source.nodes.first().copied().unwrap_or(0);
        let origin = if module {
            source.origin.clone()
        } else {
            format!("{}#script-{node}", page.url)
        };
        let result = engine.eval(bytes, &origin, module);
        flush_console(engine, page.generation, &reports);
        let _ = reports.try_send(Report::Evaluated {
            generation: page.generation,
            origin,
            node,
            result,
        });
    }
}

fn flush_console(engine: &mut Engine, generation: u64, reports: &mpsc::Sender<Report>) {
    let batch = engine.take_console();
    if !batch.messages.is_empty() || batch.dropped != 0 {
        let _ = reports.try_send(Report::Console { generation, batch });
    }
}
