//! EventStream scheduling through native logical threads and generation waits.
use alloc::sync::Arc;
use core::ffi::c_void;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, Waker as TaskWaker};
use core::time::Duration;

use futures_core::stream::Stream;

use crate::event::{
    filter::EventFilter, lock_internal_event_reader, poll_internal, read_internal,
    sys::Waker, Event, InternalEvent,
};
use crate::{io, platform::Mutex};

unsafe extern "Rust" {
    fn trueos_tokio_platform_wait_observe(key: u64) -> u32;
    fn trueos_tokio_platform_wait_after(key: u64, observed: u32, timeout_ms: u64) -> bool;
    fn trueos_tokio_platform_wake_all(key: u64) -> usize;
}
unsafe extern "C" {
    fn trueos_cabi_thread_spawn(
        stack: usize,
        start: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
        arg: *mut c_void,
        out: *mut usize,
    ) -> i32;
    fn trueos_cabi_thread_join(handle: usize) -> i32;
    fn trueos_cabi_thread_detach(handle: usize) -> i32;
}

struct State {
    queued: AtomicBool,
    shutdown: AtomicBool,
    task_waker: Mutex<Option<TaskWaker>>,
}
impl State {
    fn key(&self) -> u64 { self as *const Self as u64 ^ 0x4354_4553_0000_0000 }
    fn wake_worker(&self) { unsafe { trueos_tokio_platform_wake_all(self.key()); } }
}

unsafe extern "C" fn worker_entry(arg: *mut c_void) -> *mut c_void {
    // Admission transfers one strong reference. Native thread completion runs
    // its TLS cleanup; this reference is released before that completion.
    let state = unsafe { Arc::from_raw(arg.cast::<State>()) };
    loop {
        let observed = unsafe { trueos_tokio_platform_wait_observe(state.key()) };
        if state.shutdown.load(Ordering::Acquire) { break; }
        if !state.queued.load(Ordering::Acquire) {
            unsafe { trueos_tokio_platform_wait_after(state.key(), observed, u64::MAX); }
            continue;
        }
        while !state.shutdown.load(Ordering::Acquire) {
            match poll_internal(None, &EventFilter) {
                Ok(true) | Err(_) => break,
                Ok(false) => {}
            }
        }
        state.queued.store(false, Ordering::Release);
        let task = state.task_waker.lock().take();
        if let Some(waker) = task { waker.wake(); }
    }
    core::ptr::null_mut()
}

/// A stream of terminal events using the native TRUEOS runtime.
pub struct EventStream {
    state: Arc<State>,
    poll_waker: Waker,
    worker: usize,
}
impl core::fmt::Debug for EventStream {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EventStream").finish_non_exhaustive()
    }
}
impl Default for EventStream {
    fn default() -> Self {
        let poll_waker = lock_internal_event_reader().waker();
        let state = Arc::new(State {
            queued: AtomicBool::new(false), shutdown: AtomicBool::new(false),
            task_waker: Mutex::new(None),
        });
        let transferred = Arc::into_raw(state.clone());
        let mut worker = 0;
        let status = unsafe {
            trueos_cabi_thread_spawn(2 * 1024 * 1024, worker_entry,
                                    transferred.cast_mut().cast(), &mut worker)
        };
        if status != 0 {
            unsafe { drop(Arc::from_raw(transferred)); }
            panic!("native terminal event worker admission failed ({status})");
        }
        Self { state, poll_waker, worker }
    }
}
impl EventStream {
    pub fn new() -> Self { Self::default() }
}
impl Stream for EventStream {
    type Item = io::Result<Event>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match poll_internal(Some(Duration::ZERO), &EventFilter) {
            Ok(true) => match read_internal(&EventFilter) {
                Ok(InternalEvent::Event(event)) => Poll::Ready(Some(Ok(event))),
                Err(error) => Poll::Ready(Some(Err(error))),
                _ => unreachable!(),
            },
            Ok(false) => {
                *self.state.task_waker.lock() = Some(cx.waker().clone());
                if !self.state.queued.swap(true, Ordering::AcqRel) {
                    self.state.wake_worker();
                }
                Poll::Pending
            }
            Err(error) => Poll::Ready(Some(Err(error))),
        }
    }
}
impl Drop for EventStream {
    fn drop(&mut self) {
        self.state.shutdown.store(true, Ordering::Release);
        let _ = self.poll_waker.wake();
        self.state.wake_worker();
        // The worker owns no reference into EventStream. Joining closes its
        // lifetime before teardown; a reentrant/self drop can detach instead.
        if unsafe { trueos_cabi_thread_join(self.worker) } != 0 {
            unsafe { trueos_cabi_thread_detach(self.worker); }
        }
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::alloc_prelude::*;
    use super::*;
    use crate::event::KeyCode;
    use crate::trueos::test_support::{ATTACHED_READY, SERIAL, STATE};
    use core::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct Count(AtomicUsize);
    impl std::task::Wake for Count {
        fn wake(self: Arc<Self>) { self.0.fetch_add(1, Ordering::SeqCst); }
    }
    #[test]
    fn native_stream_wakes_for_input_retains_following_events_and_joins_its_worker_on_drop() {
        let _serial = SERIAL.lock().unwrap();
        *crate::event::INTERNAL_EVENT_READER.lock() = None;
        {
            let mut state = STATE.lock().unwrap();
            state.claim_status = 0; state.surface_status = 0; state.surface = (1, 120, 40);
            state.wait_status = 0; state.bytes.clear();
        }
        let mut stream = EventStream::new();
        let count = Arc::new(Count::default());
        let waker = TaskWaker::from(count.clone());
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut stream).poll_next(&mut cx).is_pending());
        {
            let mut state = STATE.lock().unwrap();
            state.bytes.extend(b"ab"); state.generation += 1;
        }
        ATTACHED_READY.notify_all();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while count.0.load(Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < deadline, "native event worker lost its wake");
            std::thread::yield_now();
        }
        for expected in ['a', 'b'] {
            match Pin::new(&mut stream).poll_next(&mut cx) {
                Poll::Ready(Some(Ok(Event::Key(key)))) => assert_eq!(key.code, KeyCode::Char(expected)),
                other => panic!("unexpected native stream result: {other:?}"),
            }
        }
        assert!(Pin::new(&mut stream).poll_next(&mut cx).is_pending());
        let state = stream.state.clone();
        drop(stream);
        assert!(state.shutdown.load(Ordering::Acquire));
        assert_eq!(Arc::strong_count(&state), 1, "native terminal worker outlived drop");
    }
}
