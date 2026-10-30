// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared source-backed state for the feature-enabled TRUEOS IDE.

use std::cell::RefCell;
use std::io;
use std::path::Path;
use std::rc::Rc;

use trueos_ide::{LoadedSource, ProjectBuildConfig, ProjectSummary, SourceFileInput};
#[cfg(not(target_os = "trueos"))]
use trueos_ide::SourceDatabase;
#[cfg(target_os = "trueos")]
use crate::ide_store::SourceDatabase;
#[cfg(target_os = "trueos")]
use trueos_ide::protocol;
#[cfg(any(target_os = "trueos", test))]
use trueos_ide::protocol::BuildState;
use trueos_ide::protocol::{CompilerBackend, FileContents, IdeRequest, IdeResponse, WorkspaceContext};
use trueos_ide::workspace::BuildTracker;
use crate::documents::DocumentManager;

pub type ProjectHandle = Rc<RefCell<IdeProject>>;

pub enum ProjectOpen {
    Existing,
    New,
    Configured(ProjectBuildConfig),
}

pub struct IdeSession {
    pub project: ProjectHandle,
    tracker: BuildTracker,
    backend: CompilerBackend,
    entry: String,
    #[cfg(target_os = "trueos")]
    compiler: Option<CompilerJob>,
    #[cfg(target_os = "trueos")]
    codex_download: Option<(OwnedChild, String)>,
    #[cfg(target_os = "trueos")]
    reply_errors: u8,
    #[cfg(target_os = "trueos")]
    poll_error_reported: bool,
}

pub struct IdeProject {
    database: SourceDatabase,
    pub source: LoadedSource,
    // The project lease outlives document-held handles. The kernel owns redb.
    #[cfg(target_os = "trueos")]
    _registration: OwnerRegistration,
}

impl IdeSession {
    pub fn open(database: &Path, project: &str) -> io::Result<Self> {
        Self::open_config(database, project, CompilerBackend::OdinSon, "main.odin")
    }

    pub fn open_config(database: &Path, project: &str, backend: CompilerBackend, entry: &str) -> io::Result<Self> {
        Self::open_project(database, project, ProjectOpen::Configured(ProjectBuildConfig {
            compiler: backend, entry: entry.to_owned(),
        }))
    }

    pub fn open_project(database: &Path, project: &str, mode: ProjectOpen) -> io::Result<Self> {
        #[cfg(target_os = "trueos")]
        let registration = OwnerRegistration::open(database, project)?;
        #[cfg(target_os = "trueos")]
        let tracker = BuildTracker::new_owned(workspace_epoch(), registration.endpoint);
        #[cfg(not(target_os = "trueos"))]
        let tracker = BuildTracker::new(workspace_epoch());
        if let Some(parent) = database.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut database = SourceDatabase::open(database).map_err(io::Error::other)?;
        let (source, config) = select_project(&mut database, project, mode)?;
        Ok(Self {
            project: Rc::new(RefCell::new(IdeProject {
                database, source,
                #[cfg(target_os = "trueos")]
                _registration: registration,
            })),
            tracker,
            backend: config.compiler,
            entry: config.entry,
            #[cfg(target_os = "trueos")]
            compiler: None,
            #[cfg(target_os = "trueos")]
            codex_download: None,
            #[cfg(target_os = "trueos")]
            reply_errors: 0,
            #[cfg(target_os = "trueos")]
            poll_error_reported: false,
        })
    }

    pub fn name(&self) -> String { self.project.borrow().source.revision.project.clone() }

    pub fn focus_entry(&self, documents: &mut DocumentManager) {
        documents.update_active(|doc| doc.project.is_some() && doc.path.as_deref() == Some(Path::new(&self.entry)));
    }

    pub fn list_projects(&self) -> io::Result<Vec<ProjectSummary>> {
        self.project.borrow().database.list_projects().map_err(io::Error::other)
    }

    pub fn change_project(&mut self, documents: &mut DocumentManager, name: &str, mode: ProjectOpen) -> crate::apperr::Result<()> {
        if self.name() == name && matches!(mode, ProjectOpen::Existing) {
            let paths: Vec<_> = self.project.borrow().source.files.iter().map(|file| file.path.clone()).collect();
            for path in paths { documents.add_file_path(Path::new(&path))?; }
            self.focus_entry(documents);
            return Ok(());
        }
        self.check_project_change(documents)?;
        #[cfg(target_os = "trueos")]
        {
            let mut project = self.project.borrow_mut();
            let database = project.database.path().to_owned();
            let previous = project.source.revision.project.clone();
            if let Err(error) = project._registration.change_project(&database, name) {
                let _ = project._registration.change_project(&database, &previous);
                return Err(error.into());
            }
        }
        let selected = (|| -> crate::apperr::Result<_> {
            let (source, config) = select_project(&mut self.project.borrow_mut().database, name, mode)?;
            let prepared = DocumentManager::prepare_project(self.project.clone(), &source)?;
            Ok((source, config, prepared))
        })();
        let (source, config, prepared) = match selected {
            Ok(selected) => selected,
            Err(error) => {
                #[cfg(target_os = "trueos")]
                {
                    let mut project = self.project.borrow_mut();
                    let database = project.database.path().to_owned();
                    let previous = project.source.revision.project.clone();
                    let _ = project._registration.change_project(&database, &previous);
                    self.tracker = BuildTracker::new_owned(workspace_epoch(), project._registration.endpoint);
                }
                return Err(error);
            }
        };
        #[cfg(target_os = "trueos")]
        { self.tracker = BuildTracker::new_owned(workspace_epoch(), self.project.borrow()._registration.endpoint); }
        #[cfg(not(target_os = "trueos"))]
        { self.tracker = BuildTracker::new(workspace_epoch()); }
        self.project.borrow_mut().source = source;
        self.backend = config.compiler;
        self.entry = config.entry;
        documents.replace_project(prepared);
        self.focus_entry(documents);
        Ok(())
    }

    pub fn check_project_change(&self, documents: &DocumentManager) -> io::Result<()> {
        if self.is_starting_codex() {
            return Err(io::Error::other("Wait for Codex startup before changing projects"));
        }
        if self.is_building() {
            return Err(io::Error::other("Wait for the current build before changing projects"));
        }
        if documents.has_dirty_project_documents() {
            return Err(io::Error::other("Save your project changes before changing projects"));
        }
        Ok(())
    }

    pub fn is_building(&self) -> bool { self.tracker.active().is_some() }

    pub fn is_starting_codex(&self) -> bool {
        #[cfg(target_os = "trueos")]
        return self.codex_download.is_some();
        #[cfg(not(target_os = "trueos"))]
        return false;
    }

    /// Codex has its own Shell3/Matrix lifetime. Only the download helper is
    /// owned by Edit; closing the workspace never terminates a Codex session.
    pub fn start_codex(&mut self) -> io::Result<()> {
        #[cfg(not(target_os = "trueos"))]
        return Err(io::Error::new(io::ErrorKind::Unsupported, "Codex VMs require TRUEOS"));
        #[cfg(target_os = "trueos")]
        {
            if self.is_starting_codex() { return Ok(()); }
            let script = protocol::codex_start_script(self.project.borrow()._registration.endpoint)
                .map_err(io::Error::other)?;
            match launch_codex(&script) {
                Ok(()) => Ok(()),
                Err(-2) => {
                    let child = OwnedChild::spawn_script("apps.bp", "install codex\n")?;
                    self.codex_download = Some((child, script));
                    Ok(())
                }
                Err(code) => Err(io::Error::other(format!("Cannot start Codex ({code})"))),
            }
        }
    }

    pub fn compile(&mut self) -> io::Result<u64> {
        #[cfg(not(target_os = "trueos"))]
        return Err(io::Error::new(io::ErrorKind::Unsupported, "Compiler VMs require TRUEOS"));
        #[cfg(target_os = "trueos")]
        {
            let prepared = self.tracker.prepare(Path::new(trueos_ide::workspace::BUILD_ROOT),
                &self.project.borrow().source, self.backend, &self.entry)?;
            let program = match self.backend {
                CompilerBackend::OdinSon => "odin-son.bp",
                CompilerBackend::RustcMin => "rustc-min.bp",
            };
            let path = prepared.request_path.to_str().ok_or_else(|| io::Error::other("Compiler request path must be UTF-8"))?;
            match CompilerJob::start(program, path) {
                Ok(child) => {
                    self.compiler = Some(child);
                    Ok(prepared.request.build_id)
                }
                Err(error) => {
                    self.tracker.fail(error.to_string());
                    Err(error)
                }
            }
        }
    }

    pub fn cancel_build(&mut self) -> io::Result<()> {
        if !self.tracker.cancel() {
            return Err(io::Error::other("No compiler job is active"));
        }
        #[cfg(target_os = "trueos")]
        if let Some(mut job) = self.compiler.take() { job.child.stop()?; }
        Ok(())
    }

    pub fn run(&mut self, documents: &DocumentManager) -> io::Result<String> {
        if documents.has_dirty_project_documents() {
            return Err(io::Error::other("Compile the current unsaved edits before running"));
        }
        #[cfg(not(target_os = "trueos"))]
        return Err(io::Error::new(io::ErrorKind::Unsupported, "Blueprint execution requires TRUEOS"));
        #[cfg(target_os = "trueos")]
        {
            let BuildState::Succeeded { revision_id, artifact, .. } = &self.tracker.state else {
                return Err(io::Error::other("No successful build artifact is available"));
            };
            if *revision_id != self.project.borrow().source.revision.id {
                return Err(io::Error::other("Compile the current project revision before running"));
            }
            let bytes = std::fs::read(&artifact.path)?;
            if bytes.len() as u64 != artifact.bytes || trueos_ide::workspace::sha256(&bytes) != artifact.sha256 {
                return Err(io::Error::other("Built application changed; compile it again before running"));
            }
            trueos::vshell::launch_with_destination(&artifact.path, "", trueos::vshell::LaunchDestination::NewShell3)
                .map_err(|code| io::Error::other(format!("Cannot queue application launch ({code})")))?;
            Ok(artifact.path.clone())
        }
    }

    pub fn handle_request(&mut self, documents: &mut DocumentManager, request: IdeRequest) -> IdeResponse {
        let revision_id = self.project.borrow().source.revision.id;
        match request {
            IdeRequest::Context {} => {
                let project = self.project.borrow();
                IdeResponse::Context { context: WorkspaceContext {
                    project: project.source.revision.project.clone(),
                    database: project.database.path().to_string_lossy().into_owned(),
                    compiler: self.backend,
                    entry: self.entry.clone(),
                    revision_id,
                    active_file: documents.active_project_path(),
                    open_files: documents.open_project_paths(),
                    selected_text: documents.selected_project_text(),
                    build: self.tracker.state.clone(),
                } }
            }
            IdeRequest::ReadFile { path } => match documents.project_text(&path) {
                Ok(Some(text)) => IdeResponse::File { file: FileContents { path, text, revision_id } },
                Ok(None) => IdeResponse::error("not_found", "Project source does not exist"),
                Err(error) => IdeResponse::error("read_failed", error.to_string()),
            },
            IdeRequest::WriteFile { path, expected_revision_id, expected_text, text } => {
                match documents.write_project_text(&path, expected_revision_id, expected_text.as_deref(), &text) {
                    Ok(revision_id) => IdeResponse::Saved { revision_id },
                    Err(error) => IdeResponse::error("write_failed", format!("{}", crate::state::FormatApperr::from(error))),
                }
            }
            IdeRequest::Compile { expected_revision_id } => {
                if self.is_building() {
                    return IdeResponse::error("build_running", "A compiler job is already active");
                }
                if expected_revision_id != revision_id {
                    return IdeResponse::error("stale_revision", "Project revision changed before compile");
                }
                if let Err(error) = documents.save_project_documents() {
                    return IdeResponse::error("save_failed", format!("{}", crate::state::FormatApperr::from(error)));
                }
                match self.compile() {
                    Ok(build_id) => IdeResponse::Started { build_id },
                    Err(error) => IdeResponse::error("compile_failed", error.to_string()),
                }
            }
            IdeRequest::BuildStatus {} => IdeResponse::Build { build: self.tracker.state.clone() },
            IdeRequest::CancelBuild {} => match self.cancel_build() {
                Ok(()) => IdeResponse::Cancelled {},
                Err(error) => IdeResponse::error("cancel_failed", error.to_string()),
            },
            IdeRequest::Run {} => match self.run(documents) {
                Ok(archive) => IdeResponse::LaunchQueued { archive },
                Err(error) => IdeResponse::error("run_failed", error.to_string()),
            },
        }
    }

    #[cfg(target_os = "trueos")]
    pub fn poll(&mut self, documents: &mut DocumentManager) -> io::Result<bool> {
        let codex_changed = self.poll_codex_download()?;
        let changed = self.poll_children()? || codex_changed;
        let result = self.poll_requests(documents);
        match result {
            Ok(requests) => {
                self.poll_error_reported = false;
                Ok(changed || requests)
            }
            Err(error) if !self.poll_error_reported => {
                self.poll_error_reported = true;
                Err(error)
            }
            Err(_) => Ok(changed),
        }
    }

    #[cfg(target_os = "trueos")]
    fn poll_codex_download(&mut self) -> io::Result<bool> {
        let Some((child, _)) = &mut self.codex_download else { return Ok(false) };
        let result = child.poll();
        match result {
            Ok(false) if child.started.elapsed() < std::time::Duration::from_secs(600) => return Ok(false),
            Ok(true) => {},
            other => {
                self.codex_download.take();
                return Err(other.err().unwrap_or_else(||
                    io::Error::new(io::ErrorKind::TimedOut, "Codex download timed out")));
            }
        }
        let (child, script) = self.codex_download.take().unwrap();
        launch_codex(&script).map_err(|code| {
            let mut message = format!("Cannot start Codex after download ({code})");
            if let Some(tail) = &child.diagnostics {
                message.push('\n');
                message.push_str(&String::from_utf8_lossy(&tail.bytes));
            }
            io::Error::other(message)
        })?;
        Ok(true)
    }

    #[cfg(target_os = "trueos")]
    fn poll_children(&mut self) -> io::Result<bool> {
        let Some(job) = &mut self.compiler else { return Ok(false) };
        let child = &mut job.child;
        let result = child.poll();
        match result {
            Ok(false) if child.started.elapsed() < std::time::Duration::from_secs(600) => Ok(false),
            Ok(false) => {
                let error = compiler_failure(&mut self.tracker,
                    io::Error::new(io::ErrorKind::TimedOut, "Compiler job exceeded its ten minute timeout"),
                    child.diagnostics.as_ref());
                self.compiler.take();
                Err(error)
            }
            Err(error) => {
                let error = compiler_failure(&mut self.tracker, error, child.diagnostics.as_ref());
                self.compiler.take();
                Err(error)
            }
            Ok(true) => {
                let job = self.compiler.take().unwrap();
                if let Some((program, path)) = job.pending {
                    match OwnedChild::spawn(&program, &["--ide-build", &path], true) {
                        Ok(child) => {
                            self.compiler = Some(CompilerJob { child, pending: None });
                            return Ok(true);
                        }
                        Err(error) => return Err(compiler_failure(&mut self.tracker,
                            io::Error::other(format!("Compiler download did not make {program} available: {error}")),
                            job.child.diagnostics.as_ref())),
                    }
                }
                let child = job.child;
                let artifact = match self.tracker.finish() {
                    Ok(artifact) => artifact,
                    Err(error) => return Err(compiler_failure(&mut self.tracker, error, child.diagnostics.as_ref())),
                };
                if let Err(error) = write_endpoint("vFile:appdb-install", artifact.path.as_bytes()) {
                    self.tracker.fail(error.to_string());
                    return Err(error);
                }
                Ok(true)
            }
        }
    }

    #[cfg(target_os = "trueos")]
    fn poll_requests(&mut self, documents: &mut DocumentManager) -> io::Result<bool> {
        let mut changed = false;
        for _ in 0..8 {
            let bytes = read_endpoint(protocol::POLL)?;
            if bytes.is_empty() { break; }
            let request: protocol::OwnerRequest = protocol::decode(&bytes).map_err(io::Error::other)?;
            let reply = self.handle_request(documents, request.command);
            let mut response = protocol::OwnerReply {
                endpoint: request.endpoint, client_token: request.client_token,
                request_id: request.request_id, reply,
            };
            let bytes = protocol::encode(&response).or_else(|error| {
                response.reply = IdeResponse::error("response_too_large", error);
                protocol::encode(&response)
            }).map_err(io::Error::other)?;
            if let Err(error) = write_endpoint(protocol::REPLY, &bytes)
                && self.reply_errors < 3
            {
                self.reply_errors += 1;
                crate::trueos_log(&format!("IDE reply discarded: {error}"));
            }
            changed = true;
        }
        Ok(changed)
    }
}

pub fn list_database_projects(database: &Path) -> io::Result<Vec<ProjectSummary>> {
    #[cfg(not(target_os = "trueos"))]
    match std::fs::metadata(database) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
        Ok(_) => {},
    }
    SourceDatabase::open(database).map_err(io::Error::other)?
        .list_projects().map_err(io::Error::other)
}

fn select_project(database: &mut SourceDatabase, name: &str, mode: ProjectOpen) -> io::Result<(LoadedSource, ProjectBuildConfig)> {
    let existing = database.get(name, None).map_err(io::Error::other)?;
    match (&mode, &existing) {
        (ProjectOpen::Existing, None) => return Err(io::Error::new(io::ErrorKind::NotFound, "Project no longer exists")),
        (ProjectOpen::New, Some(_)) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "A project with that name already exists")),
        _ => {},
    }
    let config = match mode {
        ProjectOpen::Configured(config) => config,
        ProjectOpen::New => ProjectBuildConfig { compiler: CompilerBackend::OdinSon, entry: "main.odin".into() },
        ProjectOpen::Existing => match database.project_build_config(name).map_err(io::Error::other)? {
            Some(config) => config,
            None => {
                let source = existing.as_ref().expect("existing project checked above");
                let entry = source.files.iter().find(|file| file.path == "main.odin")
                    .or_else(|| source.files.iter().find(|file| file.path == "main.rs"))
                    .or_else(|| {
                        let mut candidates = source.files.iter().filter(|file|
                            Path::new(&file.path).file_name().is_some_and(|name| name == "main.odin" || name == "main.rs"));
                        let first = candidates.next();
                        first.filter(|_| candidates.next().is_none())
                    })
                    .ok_or_else(|| io::Error::other("This project has no saved compiler entry point"))?;
                ProjectBuildConfig {
                    compiler: if entry.path.ends_with(".rs") { CompilerBackend::RustcMin } else { CompilerBackend::OdinSon },
                    entry: entry.path.clone(),
                }
            }
        },
    };
    if existing.is_none() {
        database.import_local(name, None, vec![SourceFileInput::new(&config.entry, Vec::new())])
            .map_err(io::Error::other)?;
    }
    let source = database.get(name, None).map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("IDE project is missing after creation"))?;
    if database.project_build_config(name).map_err(io::Error::other)?.as_ref() != Some(&config) {
        database.set_project_build_config(name, config.clone()).map_err(io::Error::other)?;
    }
    Ok((source, config))
}

/// Keep bytes until rendering so split UTF-8 sequences survive stream reads.
#[cfg(any(target_os = "trueos", test))]
struct DiagnosticTail {
    bytes: Vec<u8>,
}

#[cfg(any(target_os = "trueos", test))]
impl DiagnosticTail {
    const LIMIT: usize = 16 * 1024;

    fn new() -> Self { Self { bytes: Vec::with_capacity(Self::LIMIT) } }

    fn append(&mut self, bytes: &[u8]) {
        if bytes.len() >= Self::LIMIT {
            self.bytes.clear();
            self.bytes.extend_from_slice(&bytes[bytes.len() - Self::LIMIT..]);
        } else {
            let discard = (self.bytes.len() + bytes.len()).saturating_sub(Self::LIMIT);
            self.bytes.drain(..discard);
            self.bytes.extend_from_slice(bytes);
        }
    }
}

#[cfg(any(target_os = "trueos", test))]
fn compiler_failure(tracker: &mut BuildTracker, error: io::Error, tail: Option<&DiagnosticTail>) -> io::Error {
    let mut message = error.to_string();
    if let Some(tail) = tail {
        let output = String::from_utf8_lossy(&tail.bytes);
        if !output.trim().is_empty() {
            message.push_str("\nCompiler output (last 16 KiB):\n");
            message.push_str(&output);
        }
    }
    tracker.fail(message.clone());
    // finish() has already moved an invalid or missing result into Failed.
    if let BuildState::Failed { message: stored, .. } = &mut tracker.state {
        *stored = message.clone();
    }
    io::Error::new(error.kind(), message)
}

fn workspace_epoch() -> u64 {
    #[cfg(target_os = "trueos")]
    return trueos::clock::unix_nanos().unwrap_or(1).max(1);
    #[cfg(not(target_os = "trueos"))]
    return std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64).unwrap_or(1).max(1);
}

#[cfg(target_os = "trueos")]
struct CompilerJob {
    child: OwnedChild,
    // A missing AppDB entry is fetched by Apps before the original job retries.
    pending: Option<(String, String)>,
}

#[cfg(target_os = "trueos")]
fn launch_codex(script: &str) -> Result<(), i32> {
    trueos::vshell::launch_with_destination("codex", script, trueos::vshell::LaunchDestination::NewShell3)
}

#[cfg(target_os = "trueos")]
impl CompilerJob {
    fn start(program: &str, path: &str) -> io::Result<Self> {
        match OwnedChild::spawn(program, &["--ide-build", path], true) {
            Ok(child) => Ok(Self { child, pending: None }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let selector = program.strip_suffix(".bp").unwrap_or(program);
                let script = format!("install {selector}\n");
                let child = OwnedChild::spawn_script("apps.bp", &script)?;
                Ok(Self { child, pending: Some((program.to_owned(), path.to_owned())) })
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(target_os = "trueos")]
struct OwnedChild {
    handle: u64,
    started: std::time::Instant,
    diagnostics: Option<DiagnosticTail>,
}

#[cfg(target_os = "trueos")]
impl OwnedChild {
    fn spawn(program: &str, args: &[&str], capture_diagnostics: bool) -> io::Result<Self> {
        Self::spawn_request(program, serde_json::json!({
            "program": program,
            "args": args,
        }), capture_diagnostics)
    }

    fn spawn_script(program: &str, script: &str) -> io::Result<Self> {
        Self::spawn_request(program, serde_json::json!({ "program": program, "script": script }), true)
    }

    fn spawn_request(program: &str, request: serde_json::Value, capture_diagnostics: bool) -> io::Result<Self> {
        let request = serde_json::to_vec(&request).map_err(io::Error::other)?;
        let handle = trueos::child_hull::spawn_program(&request)
            .map_err(|code| io::Error::new(
                if code == trueos::child_hull::ERR_NOT_FOUND { io::ErrorKind::NotFound } else { io::ErrorKind::Other },
                format!("Cannot launch {program} ({code})")))?;
        Ok(Self {
            handle, started: std::time::Instant::now(),
            diagnostics: capture_diagnostics.then(DiagnosticTail::new),
        })
    }

    /// A child cannot finish until its retained stdout has been drained.
    fn poll(&mut self) -> io::Result<bool> {
        let mut buffer = [0u8; 4096];
        let mut drained = false;
        for _ in 0..16 {
            let read = trueos::child_hull::read(self.handle, &mut buffer)
                .map_err(|code| io::Error::other(format!("Child stream failed ({code})")))?;
            if read == 0 { drained = true; break; }
            if let Some(diagnostics) = &mut self.diagnostics {
                diagnostics.append(&buffer[..read]);
            }
        }
        match trueos::child_hull::status(self.handle) {
            trueos::child_hull::STATUS_STARTING | trueos::child_hull::STATUS_RUNNING
                | trueos::child_hull::STATUS_STOPPING => Ok(false),
            trueos::child_hull::STATUS_EXITED => Ok(drained),
            code => Err(io::Error::other(format!("Child lifetime was lost ({code})"))),
        }
    }

    fn stop(&mut self) -> io::Result<()> {
        if self.handle != 0 {
            trueos::child_hull::terminate(self.handle)
                .map_err(|code| io::Error::other(format!("Cannot terminate child VM ({code})")))?;
            self.handle = 0;
        }
        Ok(())
    }
}

#[cfg(target_os = "trueos")]
impl Drop for OwnedChild {
    fn drop(&mut self) { let _ = self.stop(); }
}

#[cfg(target_os = "trueos")]
impl Drop for IdeSession {
    fn drop(&mut self) {
        self.tracker.cancel();
        self.compiler.take();
    }
}

#[cfg(target_os = "trueos")]
struct OwnerRegistration { endpoint: u64 }

#[cfg(target_os = "trueos")]
impl OwnerRegistration {
    fn open(database: &Path, project: &str) -> io::Result<Self> {
        let mut lease = Self { endpoint: 0 };
        lease.change_project(database, project)?;
        Ok(lease)
    }

    fn change_project(&mut self, database: &Path, project: &str) -> io::Result<()> {
        let registration = protocol::Registration {
            version: protocol::VERSION, project: project.to_owned(),
            database: database.to_str().ok_or_else(|| io::Error::other("IDE database path must be UTF-8"))?.to_owned(),
        };
        write_endpoint(protocol::REGISTER, &protocol::encode(&registration).map_err(io::Error::other)?)?;
        // Hold the lease while discovering its host-issued endpoint. Register
        // established exclusive ownership of this project. Reject
        // ambiguous relative paths that resolve under different app roots.
        let endpoints: Vec<protocol::Endpoint> = protocol::decode(&read_endpoint(protocol::DISCOVER)?)
            .map_err(io::Error::other)?;
        let mut matching = endpoints.into_iter()
            .filter(|endpoint| endpoint.project == registration.project && endpoint.database == registration.database);
        self.endpoint = matching.next().map(|endpoint| endpoint.endpoint)
            .filter(|endpoint| *endpoint != 0)
            .ok_or_else(|| io::Error::other("Registered IDE workspace is unavailable"))?;
        if matching.next().is_some() {
            return Err(io::Error::other("IDE database path is ambiguous; use an absolute path"));
        }
        Ok(())
    }
}

#[cfg(target_os = "trueos")]
impl Drop for OwnerRegistration {
    fn drop(&mut self) { let _ = write_endpoint(protocol::UNREGISTER, &[]); }
}

#[cfg(target_os = "trueos")]
fn read_endpoint(path: &str) -> io::Result<Vec<u8>> {
    trueos::async_fs::block_on(trueos::async_fs::read_file(path.as_bytes()))
        .map_err(|code| io::Error::other(format!("IDE read {path} failed ({code})")))
}

#[cfg(target_os = "trueos")]
fn write_endpoint(path: &str, bytes: &[u8]) -> io::Result<()> {
    trueos::async_fs::block_on(trueos::async_fs::write_file(path.as_bytes(), bytes))
        .map_err(|code| if path == protocol::REGISTER && code == -9 {
            io::Error::new(io::ErrorKind::WouldBlock,
                "This project is already open in another Edit window. Use that window, or close its project before opening it here.")
        } else { io::Error::other(format!("IDE write {path} failed ({code})")) })
}

impl IdeProject {
    pub fn save_file(&mut self, path: &str, bytes: Vec<u8>) -> io::Result<()> {
        self.save_buffers(vec![(path.to_owned(), bytes)])
    }

    pub fn save_buffers(&mut self, buffers: Vec<(String, Vec<u8>)>) -> io::Result<()> {
        let mut files: Vec<_> = self.source.files.iter().map(|file|
            SourceFileInput::with_mode(&file.path, file.bytes.clone(), file.mode)).collect();
        for (path, bytes) in buffers {
            if !trueos_ide::workspace::valid_source_path(&path) || path.trim() != path
                || path.chars().any(char::is_control) || path.len() > 4096
            {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid project source path"));
            }
            if let Some(file) = files.iter_mut().find(|file| file.path == path) {
                file.bytes = bytes;
            } else {
                files.push(SourceFileInput::new(path, bytes));
            }
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let revision = self.database.save_files(&self.source.revision.project,
            self.source.revision.id, files.clone()).map_err(io::Error::other)?;
        self.source = LoadedSource {
            revision,
            files: files.into_iter().map(|file| trueos_ide::SourceFile {
                path: file.path, bytes: file.bytes, mode: file.mode,
            }).collect(),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::DocumentManager;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DATABASE: AtomicUsize = AtomicUsize::new(0);

    struct TestDatabase(PathBuf);
    impl TestDatabase {
        fn new() -> Self {
            let id = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("trueos-edit-ide-{}-{id}.redb", std::process::id()));
            let _ = std::fs::remove_file(&path);
            Self(path)
        }
    }
    impl Drop for TestDatabase {
        fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
    }

    #[test]
    fn regular_editor_can_open_project_without_losing_unrelated_edits() {
        let database = TestDatabase::new();
        assert!(list_database_projects(&database.0).unwrap().is_empty());
        assert!(!database.0.exists(), "browsing an empty catalog does not create a database");
        let mut documents = DocumentManager::default();
        documents.add_untitled().unwrap().buffer.borrow_mut().write_raw(b"keep this draft");
        let mut session = IdeSession::open_project(&database.0, "hello", ProjectOpen::New).unwrap();
        documents.load_project(session.project.clone()).unwrap();
        assert_eq!(documents.len(), 2);
        assert_eq!(documents.open_project_paths(), ["main.odin"]);
        assert_eq!(documents.project_text("main.odin").unwrap().unwrap(), "");
        assert!(documents.update_active(|doc| doc.project.is_none()));
        assert!(documents.active().unwrap().buffer.borrow().is_dirty());
        let context = session.handle_request(&mut documents, IdeRequest::Context {});
        assert!(matches!(context, IdeResponse::Context { context: WorkspaceContext { project, .. } } if project == "hello"));
    }

    #[test]
    fn project_picker_reopens_saved_compiler_and_custom_entry_without_source_changes() {
        let database = TestDatabase::new();
        {
            let mut store = SourceDatabase::open(&database.0).unwrap();
            store.import_local("rust demo", None, vec![SourceFileInput::new("src/custom.rs", b"fn main() {}\n".to_vec())]).unwrap();
        }
        let original_revision;
        {
            let session = IdeSession::open_config(&database.0, "rust demo", CompilerBackend::RustcMin, "src/custom.rs").unwrap();
            original_revision = session.project.borrow().source.revision.id;
        }
        let projects = list_database_projects(&database.0).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "rust demo");
        let session = IdeSession::open_project(&database.0, "rust demo", ProjectOpen::Existing).unwrap();
        assert_eq!(session.backend, CompilerBackend::RustcMin);
        assert_eq!(session.entry, "src/custom.rs");
        assert_eq!(session.project.borrow().source.revision.id, original_revision);
    }

    #[test]
    fn switching_projects_preserves_saved_sources_and_rejects_dirty_buffers() {
        let database = TestDatabase::new();
        {
            let mut store = SourceDatabase::open(&database.0).unwrap();
            store.import_local("second", None, vec![SourceFileInput::new("main.rs", b"fn main() {}\n".to_vec())]).unwrap();
        }
        let mut session = IdeSession::open_project(&database.0, "first", ProjectOpen::New).unwrap();
        let mut documents = DocumentManager::default();
        documents.load_project(session.project.clone()).unwrap();
        documents.active().unwrap().buffer.borrow_mut().write_raw(b"package main\n");
        assert!(session.change_project(&mut documents, "second", ProjectOpen::Existing).is_err());
        assert_eq!(session.name(), "first");
        assert!(documents.has_dirty_project_documents());
        documents.save_project_documents().unwrap();
        session.change_project(&mut documents, "second", ProjectOpen::Existing).unwrap();
        assert_eq!(session.name(), "second");
        assert_eq!(session.backend, CompilerBackend::RustcMin);
        assert_eq!(documents.open_project_paths(), ["main.rs"]);
        assert!(documents.project_text("main.odin").unwrap().is_none());
        assert_eq!(session.project.borrow().database.get("first", None).unwrap().unwrap().file("main.odin").unwrap().bytes, b"package main\n");
        assert!(matches!(session.handle_request(&mut documents, IdeRequest::Context {}),
            IdeResponse::Context { context: WorkspaceContext { project, entry, .. } } if project == "second" && entry == "main.rs"));
    }

    #[test]
    fn opening_missing_or_creating_duplicate_project_does_not_replace_workspace() {
        let database = TestDatabase::new();
        let mut session = IdeSession::open_project(&database.0, "first", ProjectOpen::New).unwrap();
        let mut documents = DocumentManager::default();
        documents.load_project(session.project.clone()).unwrap();
        let revision = session.project.borrow().source.revision.id;
        assert!(session.change_project(&mut documents, "missing", ProjectOpen::Existing).is_err());
        assert!(session.change_project(&mut documents, "first", ProjectOpen::New).is_err());
        assert_eq!(session.name(), "first");
        assert_eq!(session.project.borrow().source.revision.id, revision);
        assert_eq!(session.list_projects().unwrap().len(), 1);
        assert_eq!(documents.open_project_paths(), ["main.odin"]);
    }

    #[test]
    fn editor_new_file_saves_to_project_and_reopening_keeps_live_edits() {
        let database = TestDatabase::new();
        let mut session = IdeSession::open_project(&database.0, "demo", ProjectOpen::New).unwrap();
        let mut documents = DocumentManager::default();
        documents.load_project(session.project.clone()).unwrap();
        documents.add_untitled().unwrap().buffer.borrow_mut().write_raw(b"package helper\n");
        documents.save_project_file_as("helper/answer.odin".into()).unwrap();
        assert_eq!(session.project.borrow().source.file("helper/answer.odin").unwrap().bytes, b"package helper\n");
        assert!(!documents.active().unwrap().buffer.borrow().is_dirty());
        documents.active().unwrap().buffer.borrow_mut().write_raw(b"// unsaved\n");
        assert!(documents.save_project_file_as("main.odin".into()).is_err());
        assert!(documents.save_project_file_as("/outside.odin".into()).is_err());
        assert!(documents.active().unwrap().buffer.borrow().is_dirty());
        session.change_project(&mut documents, "demo", ProjectOpen::Existing).unwrap();
        assert_eq!(documents.active_project_path().as_deref(), Some("main.odin"));
        assert_eq!(documents.project_text("helper/answer.odin").unwrap().unwrap(), "package helper\n// unsaved\n");
        assert_eq!(documents.len(), 2);
        documents.remove_active();
        session.change_project(&mut documents, "demo", ProjectOpen::Existing).unwrap();
        assert_eq!(documents.open_project_paths().len(), 2);
        assert_eq!(documents.project_text("helper/answer.odin").unwrap().unwrap(), "package helper\n// unsaved\n");
    }

    #[test]
    fn compiler_diagnostic_tail_is_bounded_and_preserves_split_utf8() {
        let mut tail = DiagnosticTail::new();
        tail.append(&vec![b'x'; DiagnosticTail::LIMIT + 100]);
        let diagnostic = "\ncompiler failed: λ 🦀\n".as_bytes();
        let split = diagnostic.len() - 3;
        tail.append(&diagnostic[..split]);
        tail.append(&diagnostic[split..]);
        assert_eq!(tail.bytes.len(), DiagnosticTail::LIMIT);
        assert_eq!(tail.bytes.capacity(), DiagnosticTail::LIMIT);
        assert!(std::str::from_utf8(&tail.bytes).unwrap().ends_with("compiler failed: λ 🦀\n"));
        tail.append(&[0xff]);
        let message = compiler_failure(&mut BuildTracker::new(1),
            io::Error::other("stream lost"), Some(&tail)).to_string();
        assert!(message.contains("compiler failed: λ 🦀\n"));
        assert!(message.ends_with('\u{fffd}'));
    }

    #[test]
    fn missing_compiler_result_exposes_output_tail_through_build_status() {
        let database = TestDatabase::new();
        let mut session = IdeSession::open(&database.0, "demo").unwrap();
        let root = database.0.with_extension("builds");
        let source = session.project.borrow().source.clone();
        session.tracker.prepare(&root, &source, CompilerBackend::OdinSon, "main.odin").unwrap();
        let error = session.tracker.finish().unwrap_err();
        let kind = error.kind();
        let mut tail = DiagnosticTail::new();
        tail.append(b"compiler crashed before writing result.json\n");
        let reported = compiler_failure(&mut session.tracker, error, Some(&tail));
        assert_eq!(reported.kind(), kind);
        let response = session.handle_request(&mut DocumentManager::default(), IdeRequest::BuildStatus {});
        let IdeResponse::Build { build: BuildState::Failed { build_id, revision_id, message } } = response else {
            panic!("missing result must remain a failed build");
        };
        assert_eq!(build_id, 1);
        assert_eq!(revision_id, source.revision.id);
        assert!(message.contains("compiler crashed before writing result.json"));
        assert_eq!(message, reported.to_string());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_documents_save_to_shared_store_and_reopen_without_filesystem_sources() {
        let database = TestDatabase::new();
        {
            let mut store = SourceDatabase::open(&database.0).unwrap();
            store.import_local("demo", None, vec![
                SourceFileInput::new("main.rs", b"fn main() {}\n".to_vec()),
                SourceFileInput::with_mode("lib.rs", b"// helper\n".to_vec(), 0o100755),
            ]).unwrap();
        }
        {
            let session = IdeSession::open(&database.0, "demo").unwrap();
            let mut documents = DocumentManager::default();
            documents.load_project(session.project.clone()).unwrap();
            assert_eq!(documents.len(), 2);
            let doc = documents.add_file_path(Path::new("main.rs")).unwrap();
            doc.buffer.borrow_mut().write_raw(b"// edited\n");
            assert!(doc.buffer.borrow().is_dirty());
            doc.save(None).unwrap();
            assert!(!doc.buffer.borrow().is_dirty());
            documents.add_file_path(Path::new("main.rs")).unwrap();
            assert_eq!(documents.len(), 2, "opening an existing source activates its document");
        }
        let store = SourceDatabase::open(&database.0).unwrap();
        let source = store.get("demo", None).unwrap().unwrap();
        assert_eq!(source.file("main.rs").unwrap().bytes, b"// edited\nfn main() {}\n");
        assert_eq!(source.file("lib.rs").unwrap().mode, 0o100755);
        assert_eq!(source.revision.revision, 2);
    }

    #[test]
    fn stale_revision_save_keeps_buffer_dirty_and_store_contents_intact() {
        let database = TestDatabase::new();
        let session = IdeSession::open(&database.0, "demo").unwrap();
        let mut documents = DocumentManager::default();
        documents.load_project(session.project.clone()).unwrap();
        let doc = documents.active_mut().unwrap();
        doc.buffer.borrow_mut().write_raw(b"local edit\n");
        session.project.borrow_mut().database.import_local("demo", None,
            vec![SourceFileInput::new("main.odin", b"concurrent edit\n".to_vec())]).unwrap();
        assert!(doc.save(None).is_err());
        assert!(doc.buffer.borrow().is_dirty());
        let project = session.project.borrow();
        assert_eq!(project.database.get("demo", None).unwrap().unwrap().file("main.odin").unwrap().bytes,
            b"concurrent edit\n");
    }

    #[test]
    fn protocol_writes_compare_live_human_edits_and_preserve_undo() {
        let database = TestDatabase::new();
        let mut session = IdeSession::open(&database.0, "demo").unwrap();
        let mut documents = DocumentManager::default();
        documents.load_project(session.project.clone()).unwrap();
        documents.active_mut().unwrap().buffer.borrow_mut().write_raw(b"human edit\n");
        let revision = session.project.borrow().source.revision.id;
        let rejected = session.handle_request(&mut documents, IdeRequest::WriteFile {
            path: "main.odin".into(), expected_revision_id: revision,
            expected_text: Some(String::new()), text: "agent edit\n".into(),
        });
        assert!(matches!(rejected, IdeResponse::Error { .. }));
        assert_eq!(documents.project_text("main.odin").unwrap().unwrap(), "human edit\n");
        let saved = session.handle_request(&mut documents, IdeRequest::WriteFile {
            path: "main.odin".into(), expected_revision_id: revision,
            expected_text: Some("human edit\n".into()), text: "agent edit\n".into(),
        });
        assert!(matches!(saved, IdeResponse::Saved { .. }));
        assert_eq!(documents.project_text("main.odin").unwrap().unwrap(), "agent edit\n");
        documents.active_mut().unwrap().buffer.borrow_mut().undo();
        assert_eq!(documents.project_text("main.odin").unwrap().unwrap(), "human edit\n");
        assert!(documents.active().unwrap().buffer.borrow().is_dirty());
    }

    #[test]
    fn compile_preflight_saves_all_dirty_buffers_as_one_revision() {
        let database = TestDatabase::new();
        {
            let mut store = SourceDatabase::open(&database.0).unwrap();
            store.import_local("demo", None, vec![SourceFileInput::new("one.rs", Vec::new()),
                SourceFileInput::new("two.rs", Vec::new())]).unwrap();
        }
        let session = IdeSession::open(&database.0, "demo").unwrap();
        let mut documents = DocumentManager::default();
        documents.load_project(session.project.clone()).unwrap();
        for name in ["one.rs", "two.rs"] {
            documents.add_file_path(Path::new(name)).unwrap().buffer.borrow_mut().write_raw(b"// changed\n");
        }
        documents.save_project_documents().unwrap();
        let source = &session.project.borrow().source;
        assert_eq!(source.revision.revision, 2);
        assert!(source.files.iter().all(|file| file.bytes == b"// changed\n"));
        assert!(!documents.active().unwrap().buffer.borrow().is_dirty());
    }

    #[test]
    fn project_remains_accessible_after_all_documents_close() {
        let database = TestDatabase::new();
        let mut session = IdeSession::open(&database.0, "demo").unwrap();
        let mut documents = DocumentManager::default();
        documents.load_project(session.project.clone()).unwrap();
        while documents.len() != 0 { documents.remove_active(); }
        assert!(documents.open_project_paths().is_empty());
        assert!(matches!(session.handle_request(&mut documents, IdeRequest::ReadFile {
            path: "main.odin".into(),
        }), IdeResponse::File { .. }));
        let revision = session.project.borrow().source.revision.id;
        assert!(matches!(session.handle_request(&mut documents, IdeRequest::WriteFile {
            path: "main.odin".into(), expected_revision_id: revision,
            expected_text: Some(String::new()), text: "reopened\n".into(),
        }), IdeResponse::Saved { .. }));
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.project_text("main.odin").unwrap().unwrap(), "reopened\n");
    }

    #[test]
    fn adding_an_earlier_source_keeps_lookup_sorted_and_saves_live_snapshot() {
        let database = TestDatabase::new();
        let mut session = IdeSession::open(&database.0, "demo").unwrap();
        let mut documents = DocumentManager::default();
        documents.load_project(session.project.clone()).unwrap();
        documents.active_mut().unwrap().buffer.borrow_mut().write_raw(b"human edit\n");
        let revision = session.project.borrow().source.revision.id;
        assert!(matches!(session.handle_request(&mut documents, IdeRequest::WriteFile {
            path: "a.odin".into(), expected_revision_id: revision,
            expected_text: None, text: "new module\n".into(),
        }), IdeResponse::Saved { .. }));
        let project = session.project.borrow();
        assert_eq!(project.source.file("a.odin").unwrap().bytes, b"new module\n");
        assert_eq!(project.source.file("main.odin").unwrap().bytes, b"human edit\n");
        assert_eq!(project.source.revision.revision, 2);
        assert_eq!(documents.len(), 2);
    }
}
