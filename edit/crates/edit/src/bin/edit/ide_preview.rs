//! Edit owns the preview server child; Hull owns its normal browser lifetime.
use std::path::PathBuf;
use std::time::Duration;
use std::{fs, io};

use trueos_ide::html_preview::{self, Config, Ready};
use trueos_ide::workspace;

use super::{IdeSession, OwnedChild, workspace_epoch};

pub(super) struct Preview {
    child: OwnedChild,
    config: Config,
    pub url: Option<String>,
    open_browser: bool,
}

impl IdeSession {
    pub(super) fn start_preview(&mut self) -> io::Result<Option<String>> {
        if self.project.borrow().source.file(&self.entry).is_none() {
            return Err(io::Error::other("HTML entry does not exist in the saved project"));
        }
        if let Some(preview) = &mut self.preview {
            workspace::publish_html(
                std::path::Path::new(&preview.config.root),
                &self.project.borrow().source,
            )?;
            preview.open_browser = true;
            return Ok(preview.url.clone());
        }
        let project = self.project.borrow();
        let owner = project._registration.endpoint;
        let base = PathBuf::from(html_preview::ROOT).join(format!("{}-{owner}", workspace_epoch()));
        let root = base.join("public");
        workspace::publish_html(&root, &project.source)?;
        let ready = base.join("ready.json");
        match fs::remove_file(&ready) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let config = Config {
            root: root.to_string_lossy().into_owned(),
            ready: ready.to_string_lossy().into_owned(),
            entry: self.entry.clone(),
        };
        let child = OwnedChild::spawn_script(
            "html-preview.bp",
            &config.script().map_err(io::Error::other)?,
        )?;
        drop(project);
        self.project.borrow_mut().preview_root = Some(root);
        self.preview = Some(Preview { child, config, url: None, open_browser: true });
        Ok(None)
    }

    pub(super) fn poll_preview(&mut self) -> io::Result<bool> {
        let Some(preview) = &mut self.preview else { return Ok(false) };
        let result = (|| {
            if preview.child.poll()? {
                return Err(io::Error::other("HTML preview server exited"));
            }
            let mut changed = false;
            if preview.url.is_none() {
                match fs::read(&preview.config.ready) {
                    Ok(bytes) => {
                        let ready: Ready =
                            serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                        // Refuse stale/misdirected readiness before launching a browser.
                        let address = ready
                            .url
                            .strip_prefix("http://127.0.0.1:")
                            .and_then(|rest| rest.split_once('/'))
                            .and_then(|(port, _)| port.parse::<u16>().ok());
                        if ready.protocol != html_preview::PROTOCOL
                            || ready.root != preview.config.root
                            || address.is_none_or(|port| port == 0)
                            || ready.url.chars().any(char::is_whitespace)
                        {
                            return Err(io::Error::other("Invalid HTML preview readiness"));
                        }
                        preview.url = Some(ready.url);
                        changed = true;
                    }
                    Err(error)
                        if error.kind() == io::ErrorKind::NotFound
                            && preview.child.started.elapsed() < Duration::from_secs(15) => {}
                    Err(error) => {
                        return Err(io::Error::other(format!(
                            "HTML preview did not become ready: {error}"
                        )));
                    }
                }
            }
            if preview.open_browser
                && let Some(url) = &preview.url
            {
                trueos::vshell::launch_with_destination(
                    "hull",
                    &format!("open {url}\n"),
                    trueos::vshell::LaunchDestination::NewShell3,
                )
                .map_err(|code| io::Error::other(format!("Cannot open Hull preview ({code})")))?;
                preview.open_browser = false;
            }
            Ok(changed)
        })();
        if result.is_err() {
            self.preview.take();
            self.project.borrow_mut().preview_root = None;
        }
        result
    }
}
