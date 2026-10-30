//! Native source-store client. The kernel owns the shared redb file.
use std::{io, path::{Path, PathBuf}};
use trueos_ide::{LoadedSource, ProjectBuildConfig, ProjectSummary, RevisionSummary, SourceFileInput};
use trueos_ide::store_protocol::{self, Command, Reply, Request};

pub struct SourceDatabase { path: PathBuf }

impl SourceDatabase {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_owned();
        if path.to_str().is_none() { return Err(io::Error::other("Source database path must be UTF-8")); }
        Ok(Self { path })
    }
    pub fn path(&self) -> &Path { &self.path }
    fn call(&self, command: Command) -> io::Result<Reply> {
        let bytes = serde_json::to_vec(&Request {
            database: self.path.to_str().unwrap().to_owned(), command,
        }).map_err(io::Error::other)?;
        if bytes.len() > store_protocol::MAX_FRAME_BYTES {
            return Err(io::Error::other("Project source request exceeds the source service limit"));
        }
        trueos::async_fs::block_on(trueos::async_fs::write_file(store_protocol::REQUEST.as_bytes(), &bytes))
            .map_err(|code| io::Error::other(format!("Source service request failed ({code})")))?;
        let bytes = trueos::async_fs::block_on(trueos::async_fs::read_file(store_protocol::REPLY.as_bytes()))
            .map_err(|code| io::Error::other(format!("Source service reply failed ({code})")))?;
        let reply = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if let Reply::Error { code, message } = reply {
            return Err(io::Error::other(format!("{message} ({code})")));
        }
        Ok(reply)
    }
    pub fn list_projects(&self) -> io::Result<Vec<ProjectSummary>> {
        match self.call(Command::List {})? {
            Reply::Projects { projects } => Ok(projects), _ => Err(unexpected()),
        }
    }
    pub fn get(&self, project: &str, revision: Option<u64>) -> io::Result<Option<LoadedSource>> {
        match self.call(Command::Get { project: project.into(), revision })? {
            Reply::Source { source } => Ok(source), _ => Err(unexpected()),
        }
    }
    pub fn project_build_config(&self, project: &str) -> io::Result<Option<ProjectBuildConfig>> {
        match self.call(Command::GetConfig { project: project.into() })? {
            Reply::Config { config } => Ok(config), _ => Err(unexpected()),
        }
    }
    pub fn set_project_build_config(&mut self, project: &str, config: ProjectBuildConfig) -> io::Result<()> {
        match self.call(Command::SetConfig { project: project.into(), config })? {
            Reply::Done {} => Ok(()), _ => Err(unexpected()),
        }
    }
    pub fn import_local(&mut self, project: &str, _: Option<&str>, files: Vec<SourceFileInput>) -> io::Result<RevisionSummary> {
        match self.call(Command::Create { project: project.into(), files })? {
            Reply::Saved { revision } => Ok(revision), _ => Err(unexpected()),
        }
    }
    pub fn save_files(&mut self, project: &str, expected_revision_id: i64, files: Vec<SourceFileInput>) -> io::Result<RevisionSummary> {
        match self.call(Command::Save { project: project.into(), expected_revision_id, files })? {
            Reply::Saved { revision } => Ok(revision), _ => Err(unexpected()),
        }
    }
}
fn unexpected() -> io::Error { io::Error::other("Unexpected source service reply") }
