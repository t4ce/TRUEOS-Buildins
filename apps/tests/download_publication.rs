//! Verify the production publication pipeline before any AppDB/launch caller.
extern crate alloc;
extern crate self as trueos;

use std::{cell::RefCell, future::Future, task::{Context, Poll, Waker}};
#[derive(Default)]
struct State {
    calls: Vec<&'static str>,
    fetch_error: bool,
    directory_error: bool,
    write_error: bool,
    body: Vec<u8>,
    published: Vec<u8>,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
fn state(f: impl FnOnce(&mut State)) { STATE.with(|state| f(&mut state.borrow_mut())); }
fn reset() { state(|state| *state = State::default()); }
fn run<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) { Poll::Ready(value) => value, Poll::Pending => panic!("fixture unexpectedly parked") }
}

pub mod time {
    pub struct Instant;
    impl Instant {
        pub fn now() -> Self { Self }
        pub fn elapsed(&self) -> core::time::Duration { core::time::Duration::ZERO }
    }
}
pub mod content_identity {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ContentTypeId;
    impl ContentTypeId { pub const TRUEOS_BLUEPRINT: Self = Self; }
}
pub mod async_fs {
    pub async fn create_dir_all(path: &[u8]) -> Result<(), i32> {
        assert_eq!(path, b"common/dl/apps");
        super::STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.calls.push("mkdir");
            if state.directory_error { Err(-4) } else { Ok(()) }
        })
    }
    pub async fn write_file_typed_checked(path: &[u8], bytes: &[u8], identity: super::content_identity::ContentTypeId, create_new: bool, digest: Option<[u8; 32]>) -> Result<(), i32> {
        assert_eq!(path, b"common/dl/apps/Paper.bp");
        assert_eq!(identity, super::content_identity::ContentTypeId::TRUEOS_BLUEPRINT);
        assert!(!create_new && digest.is_none());
        super::STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.calls.push("publish");
            if state.write_error { return Err(-2); }
            state.published = bytes.to_vec();
            Ok(())
        })
    }
}
mod fetch {
    pub const ARCHIVE_LIMIT: usize = 128 * 1024 * 1024;
    pub async fn bytes(_: &str, limit: usize) -> Result<Vec<u8>, String> {
        assert_eq!(limit, ARCHIVE_LIMIT);
        super::STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.calls.push("fetch");
            if state.fetch_error { Err("native transport failed (-115)".into()) } else { Ok(state.body.clone()) }
        })
    }
}
mod pullbot { pub fn report(_: &str) {} }
#[path = "../src/catalog.rs"]
mod catalog;
#[path = "../src/download.rs"]
mod download;

fn entry(hash: Option<&str>) -> catalog::OnlineApp {
    let suffix = hash.map_or(String::new(), |hash| format!("§§{hash}"));
    catalog::parse(&format!("<li><a href=\"Paper.bp{suffix}\">Paper.bp{suffix}</a></li>")).remove(0)
}

#[test]
fn failed_native_transport_does_not_create_or_publish_anything() {
    reset();
    state(|state| state.fetch_error = true);
    assert!(run(download::download(&entry(None))).is_err());
    state(|state| assert_eq!(state.calls, ["fetch"]));
}

#[test]
fn sha_mismatch_does_not_create_or_publish_anything() {
    reset();
    state(|state| state.body = b"changed".to_vec());
    let empty_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert!(run(download::download(&entry(Some(empty_hash)))).unwrap_err().contains("SHA-256"));
    state(|state| assert_eq!(state.calls, ["fetch"]));
}

#[test]
fn publication_error_never_returns_a_path_for_install_or_launch() {
    reset();
    state(|state| {
        state.body = b"new".to_vec();
        state.write_error = true;
        state.published = b"previous".to_vec();
    });
    assert!(run(download::download(&entry(None))).unwrap_err().contains("Save download"));
    state(|state| {
        assert_eq!(state.calls, ["fetch", "mkdir", "publish"]);
        assert_eq!(state.published, b"previous");
    });
}

#[test]
fn path_is_returned_only_after_successful_typed_publication() {
    reset();
    state(|state| state.body = b"legacy catalog without a hash".to_vec());
    assert_eq!(run(download::download(&entry(None))).unwrap(), "common/dl/apps/Paper.bp");
    state(|state| {
        assert_eq!(state.calls, ["fetch", "mkdir", "publish"]);
        assert_eq!(state.published, b"legacy catalog without a hash");
    });
}

#[test]
fn directory_failure_prevents_archive_publication() {
    reset();
    state(|state| state.directory_error = true);
    assert!(run(download::download(&entry(None))).is_err());
    state(|state| assert_eq!(state.calls, ["fetch", "mkdir"]));
}
