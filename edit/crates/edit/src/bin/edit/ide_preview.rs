//! IDEdit owns its development server child; Hull owns the browser window.
use std::io;
use std::time::Duration;

use trueos_ide::html_preview::{Config, PROGRAM, PROTOCOL, READY_PREFIX, Ready};

use super::{IdeSession, OwnedChild};

pub(super) struct Preview {
    child: OwnedChild,
    downloading: bool,
    config: Config,
    pub url: Option<String>,
    open_browser: bool,
}
impl IdeSession {
    pub(super) fn start_preview(&mut self) -> io::Result<Option<String>> {
        let project = self.project.borrow();
        if project.source.file(&self.entry).is_none() {
            return Err(io::Error::other("HTML entry does not exist in the saved project"));
        }
        if let Some(preview) = &mut self.preview {
            preview.open_browser = true;
            return Ok(preview.url.clone());
        }
        let config = Config {
            endpoint: project._registration.endpoint,
            database: project
                .database
                .path()
                .to_str()
                .ok_or_else(|| io::Error::other("IDE database must be UTF-8"))?
                .into(),
            project: project.source.revision.project.clone(),
            entry: self.entry.clone(),
        };
        let script = config.script().map_err(io::Error::other)?;
        let (child, downloading) = match OwnedChild::spawn_script(PROGRAM, &script) {
            Ok(child) => (child, false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                (OwnedChild::spawn_script("apps.bp", "install idedit_htmlbuild_srv\n")?, true)
            }
            Err(error) => return Err(error),
        };
        self.preview = Some(Preview { child, downloading, config, url: None, open_browser: true });
        Ok(None)
    }

    pub(super) fn poll_preview(&mut self) -> io::Result<bool> {
        let Some(preview) = &mut self.preview else { return Ok(false) };
        let result = (|| {
            let exited = preview.child.poll()?;
            if preview.downloading {
                if !exited {
                    if preview.child.started.elapsed() >= Duration::from_secs(600) {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "IDEdit HTML tool download timed out",
                        ));
                    }
                    return Ok(false);
                }
                preview.child = OwnedChild::spawn_script(
                    PROGRAM,
                    &preview.config.script().map_err(io::Error::other)?,
                )
                .map_err(|error| {
                    io::Error::other(format!(
                        "Cannot start IDEdit HTML tool after download: {error}\n{}",
                        preview
                            .child
                            .diagnostics
                            .as_ref()
                            .map(|tail| String::from_utf8_lossy(&tail.bytes))
                            .unwrap_or_default()
                    ))
                })?;
                preview.downloading = false;
                return Ok(true);
            }
            if exited {
                return Err(io::Error::other("IDEdit HTML development server exited"));
            }
            let mut changed = false;
            if preview.url.is_none() {
                let output = preview
                    .child
                    .diagnostics
                    .as_ref()
                    .map(|tail| String::from_utf8_lossy(&tail.bytes))
                    .unwrap_or_default();
                for line in output.lines() {
                    let Some(record) = line.strip_prefix(READY_PREFIX) else { continue };
                    // A record split across child reads is retried on the next poll.
                    let Ok(ready) = serde_json::from_str::<Ready>(record) else { continue };
                    let address = ready
                        .url
                        .strip_prefix("http://127.0.0.1:")
                        .and_then(|rest| rest.split_once('/'))
                        .and_then(|(port, _)| port.parse::<u16>().ok());
                    if ready.protocol != PROTOCOL
                        || ready.endpoint != preview.config.endpoint
                        || address.is_none_or(|port| port == 0)
                        || ready.url.chars().any(char::is_whitespace)
                    {
                        return Err(io::Error::other("Invalid IDEdit HTML readiness"));
                    }
                    preview.url = Some(ready.url);
                    changed = true;
                    break;
                }
                if preview.url.is_none()
                    && preview.child.started.elapsed() >= Duration::from_secs(15)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "IDEdit HTML server did not become ready",
                    ));
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
        }
        result
    }
}
