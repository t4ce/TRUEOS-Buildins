//! One idle Tokio local task. Page arrival never authorizes execution.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use hull_quickjs::Engine;
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
}

#[derive(Clone)]
struct Page {
    revision: u64,
    generation: u64,
    url: String,
    sources: Arc<[ScriptSource]>,
    requested: bool,
}

pub(super) struct Runtime {
    local: LocalSet,
    runtime: runtime::Runtime,
    task: JoinHandle<()>,
    pages: watch::Sender<Option<Page>>,
    ticks: watch::Sender<u64>,
    reports: mpsc::Receiver<Report>,
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
        let (reports_tx, reports) = mpsc::channel(64);
        let task = local.spawn_local(worker(page_rx, tick_rx, reports_tx));
        let mut service = Self {
            local,
            runtime,
            task,
            pages,
            ticks,
            reports,
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
        self.pages.send_replace(Some(Page {
            revision: self.revision,
            generation,
            url: url.into(),
            sources: sources.into(),
            requested: false,
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
        self.pages.send_replace(Some(page));
        Ok(())
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
) {
    let _ = reports.try_send(Report::Online);
    let mut page: Option<Page> = None;
    let mut engine: Option<Engine> = None;
    let mut started = false;
    let mut next = 0;
    while ticks.changed().await.is_ok() {
        if pages.has_changed().unwrap_or(false) {
            let incoming = pages.borrow_and_update().clone();
            if incoming.as_ref().map(|p| p.revision) != page.as_ref().map(|p| p.revision) {
                engine = None; // Context, globals, pending promises and limits all go together.
                next = 0;
                started = false;
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
        if !page.requested {
            continue;
        }
        if !started {
            started = true;
            match Engine::new() {
                Ok(value) => engine = Some(value),
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
            continue;
        }
        let Some(source) = page.sources.get(next) else {
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
        let _ = reports.try_send(Report::Evaluated {
            generation: page.generation,
            origin,
            node,
            result,
        });
    }
}
