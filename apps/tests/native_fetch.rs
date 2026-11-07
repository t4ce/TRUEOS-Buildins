//! Exercise production download ownership against native-service outcomes.
extern crate alloc;
extern crate self as trueos;

use std::{cell::RefCell, collections::VecDeque, future::Future, task::{Context, Poll, Waker}};

#[derive(Default)]
struct State {
    now: u64,
    start_error: Option<i32>,
    statuses: VecDeque<Result<usize, i32>>,
    body: Vec<u8>,
    read_error: Option<i32>,
    reads: usize,
    discards: usize,
    admitted: Option<(u32, usize)>,
    logs: Vec<String>,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
fn state(f: impl FnOnce(&mut State)) { STATE.with(|state| f(&mut state.borrow_mut())); }
fn reset() { state(|state| *state = State::default()); }
fn run<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    loop { if let Poll::Ready(value) = future.as_mut().poll(&mut context) { return value; } }
}

pub mod time {
    pub struct Instant(u64);
    impl Instant {
        pub fn now() -> Self { Self(super::STATE.with(|state| state.borrow().now)) }
        pub fn elapsed(&self) -> core::time::Duration {
            core::time::Duration::from_millis(super::STATE.with(|state| state.borrow().now - self.0))
        }
    }
    pub async fn sleep(duration: core::time::Duration) {
        let mut parked = false;
        core::future::poll_fn(|_| {
            if parked {
                super::state(|state| state.now += duration.as_millis() as u64);
                core::task::Poll::Ready(())
            } else { parked = true; core::task::Poll::Pending }
        }).await;
    }
}
pub mod netfs {
    pub fn fetch_bytes_with_options(_: &[u8], timeout: u32, limit: usize) -> Result<u32, i32> {
        super::STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.admitted = Some((timeout, limit));
            match state.start_error { Some(code) => Err(code), None => Ok(7) }
        })
    }
    pub fn fetch_bytes_result_len(_: u32) -> Result<usize, i32> {
        super::STATE.with(|state| state.borrow_mut().statuses.pop_front().unwrap_or(Err(-8)))
    }
    pub fn fetch_bytes_read_bounded(_: u32, limit: usize) -> Result<Vec<u8>, i32> {
        super::STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.reads += 1;
            if let Some(code) = state.read_error { return Err(code); }
            assert!(state.body.len() <= limit);
            Ok(state.body.clone())
        })
    }
    pub fn fetch_bytes_discard(id: u32) -> i32 {
        assert_eq!(id, 7);
        super::state(|state| state.discards += 1);
        0
    }
}
mod pullbot {
    pub fn report(message: &str) { super::state(|state| state.logs.push(message.into())); }
}
#[path = "../src/fetch.rs"]
mod fetch;

#[test]
fn successful_catalog_keeps_limits_and_yields_while_waiting() {
    reset();
    state(|state| {
        state.statuses.extend([Err(-8), Ok(3)]);
        state.body = b"abc".to_vec();
    });
    assert_eq!(run(fetch::bytes("https://trueos.eu/apps", fetch::CATALOG_LIMIT)).unwrap(), b"abc");
    state(|state| {
        assert_eq!(state.admitted, Some((90_000, 1024 * 1024)));
        assert_eq!((state.reads, state.discards, state.now), (1, 1, 25));
    });
}

#[test]
fn owned_future_drop_releases_operation_immediately() {
    reset();
    let mut future = Box::pin(fetch::bytes("https://trueos.eu/file.bp", fetch::ARCHIVE_LIMIT));
    assert!(future.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    drop(future);
    state(|state| assert_eq!(state.discards, 1));
}

#[test]
fn admission_failure_does_not_discard_an_unowned_identifier() {
    reset();
    state(|state| state.start_error = Some(-4));
    assert!(run(fetch::bytes("bad", 32)).is_err());
    state(|state| assert_eq!((state.reads, state.discards), (0, 0)));
}

#[test]
fn transport_failure_and_failed_body_read_both_release_operation() {
    for reading in [false, true] {
        reset();
        state(|state| {
            state.statuses.push_back(if reading { Ok(3) } else { Err(-12) });
            state.read_error = Some(-2);
        });
        assert!(run(fetch::bytes("https://example.invalid/secret?token=hidden", 32)).is_err());
        state(|state| {
            assert_eq!(state.discards, 1);
            assert!(state.logs.iter().all(|message| !message.contains("token") && !message.contains("secret")));
        });
    }
}

#[test]
fn oversize_length_is_rejected_before_a_body_allocation() {
    reset();
    state(|state| state.statuses.push_back(Ok(33)));
    assert!(run(fetch::bytes("https://trueos.eu/file.bp", 32)).unwrap_err().contains("size limit"));
    state(|state| assert_eq!((state.reads, state.discards), (0, 1)));
}

#[test]
fn a_missing_or_stalled_operation_has_a_bounded_lifetime() {
    reset();
    assert!(run(fetch::bytes("https://trueos.eu/file.bp", 32)).unwrap_err().contains("timed out"));
    state(|state| {
        assert_eq!(state.now, 90_000);
        assert_eq!(state.discards, 1);
    });
}
