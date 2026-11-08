//! Exercise production bundle fetch and pullbot with failed/missing members.
extern crate alloc;
extern crate self as trueos;

use std::{
    cell::RefCell,
    future::Future,
    task::{Context, Poll, Waker},
};
const CATALOG: &str = "https://trueos.eu/apps";

#[derive(Default)]
struct State {
    catalog: String,
    fail_fetch: Option<String>,
    fail_import: bool,
    fetched: Vec<String>,
    published: Vec<String>,
    imports: Vec<String>,
    launches: Vec<(String, String)>,
    errors: Vec<String>,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
fn state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(&mut s.borrow_mut()))
}
fn reset(names: &[&str]) {
    state(|s| {
        *s = State {
            catalog: names
                .iter()
                .map(|name| format!("<li><a href=\"{name}.bp\">{name}.bp</a></li>"))
                .collect(),
            ..State::default()
        }
    });
}
fn run<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("fixture unexpectedly parked"),
    }
}
fn error_output(error: &str) {
    state(|s| s.errors.push(error.into()));
}
pub mod logl {
    pub mod level {
        pub const IMPORTANT: u8 = 1;
    }
    pub fn log_record(_: u8, _: &str, _: core::fmt::Arguments<'_>) -> Result<(), i32> {
        Ok(())
    }
}
pub mod time {
    pub struct Instant;
    impl Instant {
        pub fn now() -> Self {
            Self
        }
        pub fn elapsed(&self) -> core::time::Duration {
            core::time::Duration::ZERO
        }
    }
    pub async fn sleep(_: core::time::Duration) {}
}
pub mod content_identity {
    pub struct ContentTypeId;
    impl ContentTypeId {
        pub const TRUEOS_BLUEPRINT: Self = Self;
    }
}
pub mod async_fs {
    pub async fn read_file(path: &[u8]) -> Result<Vec<u8>, i32> {
        assert_eq!(path, trueos_appbundle::VFILE);
        Ok(include_bytes!("../../../TRUEOS-Blueprints/appbundle.json").to_vec())
    }
    pub async fn create_dir_all(_: &[u8]) -> Result<(), i32> {
        Ok(())
    }
    pub async fn write_file_typed_checked(
        path: &[u8],
        _: &[u8],
        _: super::content_identity::ContentTypeId,
        _: bool,
        _: Option<[u8; 32]>,
    ) -> Result<(), i32> {
        super::state(|s| s.published.push(String::from_utf8(path.to_vec()).unwrap()));
        Ok(())
    }
    pub async fn write_file(path: &[u8], bytes: &[u8]) -> Result<(), i32> {
        assert_eq!(path, b"vFile:appdb-install");
        super::state(|s| {
            if s.fail_import {
                return Err(-2);
            }
            s.imports.push(String::from_utf8(bytes.to_vec()).unwrap());
            Ok(())
        })
    }
}
pub mod vshell {
    pub enum LaunchDestination {
        Headless,
        NewShell3,
        CurrentShell,
    }
    pub fn launch_with_destination(
        path: &str,
        script: &str,
        _: LaunchDestination,
    ) -> Result<(), i32> {
        super::state(|s| s.launches.push((path.into(), script.into())));
        Ok(())
    }
}
mod fetch {
    pub const CATALOG_LIMIT: usize = 1024 * 1024;
    pub const ARCHIVE_LIMIT: usize = 128 * 1024 * 1024;
    pub async fn bytes(url: &str, _: usize) -> Result<Vec<u8>, String> {
        super::state(|s| {
            if url == super::CATALOG {
                return Ok(s.catalog.as_bytes().to_vec());
            }
            let name = url.rsplit('/').next().unwrap().to_string();
            s.fetched.push(name.clone());
            if s.fail_fetch.as_deref() == Some(&name) {
                return Err("Download HTTP failed (-12)".into());
            }
            Ok(b"archive fixture".to_vec())
        })
    }
}
#[path = "../src/bundle.rs"]
mod bundle;
#[path = "../src/catalog.rs"]
mod catalog;
#[path = "../src/download.rs"]
mod download;
#[path = "../src/pullbot.rs"]
mod pullbot;
#[path = "../src/script.rs"]
mod script;

#[test]
fn missing_sqlite_fails_before_downloading_or_launching_codex() {
    reset(&["codex"]);
    run(pullbot::drain(b"pull codex\n"));
    state(|s| {
        assert_eq!(
            s.errors,
            ["codex: required sqlite.bp unavailable in app catalog"]
        );
        assert!(s.fetched.is_empty() && s.imports.is_empty() && s.launches.is_empty());
    });
}

#[test]
fn either_unavailable_member_reports_the_member_and_prevents_install_and_launch() {
    for member in ["sqlite.bp", "codex.bp"] {
        reset(&["codex", "sqlite"]);
        state(|s| s.fail_fetch = Some(member.into()));
        run(pullbot::drain(b"pull codex\n"));
        state(|s| {
            assert_eq!(
                s.errors,
                [format!(
                    "codex: {member} unavailable: Download HTTP failed (-12)"
                )]
            );
            assert!(s.imports.is_empty() && s.launches.is_empty());
        });
    }
}

#[test]
fn bundle_installs_both_before_launching_only_codex_with_original_script() {
    reset(&["codex", "sqlite"]);
    run(pullbot::drain(b"launch --sh3 codex -- ide-endpoint 42\n"));
    state(|s| {
        assert!(s.errors.is_empty());
        assert_eq!(s.fetched, ["sqlite.bp", "codex.bp"]);
        assert_eq!(
            s.imports,
            ["common/dl/apps/sqlite.bp\ncommon/dl/apps/codex.bp"]
        );
        assert_eq!(
            s.launches,
            [("common/dl/apps/codex.bp".into(), "ide-endpoint 42\n".into())]
        );
    });
}

#[test]
fn install_only_imports_the_complete_bundle_without_launching() {
    reset(&["codex", "sqlite"]);
    run(pullbot::drain(b"install codex\n"));
    state(|s| {
        assert_eq!(s.imports.len(), 1);
        assert!(s.errors.is_empty() && s.launches.is_empty());
    });
}

#[test]
fn sqlite_on_its_own_does_not_request_codex() {
    reset(&["sqlite"]);
    run(pullbot::drain(b"install sqlite\n"));
    state(|s| {
        assert_eq!(s.fetched, ["sqlite.bp"]);
        assert_eq!(s.imports, ["common/dl/apps/sqlite.bp"]);
        assert!(s.errors.is_empty() && s.launches.is_empty());
    });
}

#[test]
fn interactive_fetch_returns_the_same_missing_member_status() {
    reset(&["codex"]);
    let apps = state(|s| catalog::parse(&s.catalog));
    assert_eq!(
        run(bundle::download(&apps, &apps[0], false)).unwrap_err(),
        "codex: required sqlite.bp unavailable in app catalog"
    );
}

#[test]
fn failed_verification_and_failed_import_do_not_launch() {
    reset(&["codex", "sqlite"]);
    let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    state(|s| {
        s.catalog = s
            .catalog
            .replace("href=\"codex.bp\"", &format!("href=\"codex.bp§§{hash}\""))
    });
    run(pullbot::drain(b"pull codex\n"));
    state(|s| {
        assert!(s.errors[0].contains("codex.bp unavailable: Blueprint SHA-256 mismatch"));
        assert!(s.imports.is_empty() && s.launches.is_empty());
    });
    reset(&["codex", "sqlite"]);
    state(|s| s.fail_import = true);
    run(pullbot::drain(b"pull codex\n"));
    state(|s| {
        assert!(s.errors[0].contains("AppDB bundle import failed"));
        assert!(s.imports.is_empty() && s.launches.is_empty());
    });
}
