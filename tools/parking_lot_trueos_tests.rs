//! Host tests of the actual vendored TRUEOS ThreadParker implementation.
//!
//! test_parking_lot_trueos.py inserts the production module and trait definitions
//! below. Native wait/wake calls are modeled here; the parker algorithm is not.
#![allow(dead_code, unused_imports)]

// @PRODUCTION_PARKER_MODULE@

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use thread_parker::{ThreadParker, ThreadParkerT, UnparkHandleT};

const TEST_LIMIT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Observed,
    WaitEntry,
    Registered,
}

#[derive(Clone, Copy, Debug)]
struct Event {
    kind: Kind,
    key: u64,
    scope: u16,
    timeout_ms: u64,
}

struct Probe {
    events: mpsc::Sender<Event>,
    pause: Option<(Kind, mpsc::Receiver<()>)>,
}

thread_local! {
    static SCOPE: Cell<u16> = const { Cell::new(0) };
    static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
}

fn event(kind: Kind, key: u64, timeout_ms: u64) {
    PROBE.with(|probe| {
        let mut probe = probe.borrow_mut();
        if let Some(probe) = probe.as_mut() {
            let _ = probe.events.send(Event {
                kind,
                key,
                scope: SCOPE.get(),
                timeout_ms,
            });
            if probe.pause.as_ref().map(|pause| pause.0) == Some(kind) {
                let (_, gate) = probe.pause.take().unwrap();
                gate.recv_timeout(TEST_LIMIT)
                    .expect("test did not release platform checkpoint");
            }
        }
    });
}

#[derive(Default)]
struct QueueState {
    generation: u32,
    waiting: usize,
}

#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
    changed: Condvar,
}

fn queue(key: u64) -> Arc<Queue> {
    type Queues = BTreeMap<(u16, u64), Arc<Queue>>;
    static QUEUES: OnceLock<Mutex<Queues>> = OnceLock::new();
    QUEUES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap()
        .entry((SCOPE.get(), key))
        .or_default()
        .clone()
}

#[no_mangle]
extern "Rust" fn trueos_tokio_platform_wait_observe(key: u64) -> u32 {
    let generation = queue(key).state.lock().unwrap().generation;
    // The checkpoint is after observing so tests can insert a wake exactly
    // between the observation and the parker's subsequent state recheck.
    event(Kind::Observed, key, 0);
    generation
}

#[no_mangle]
extern "Rust" fn trueos_tokio_platform_wait_after(
    key: u64,
    observed: u32,
    timeout_ms: u64,
) -> bool {
    // This checkpoint inserts a wake after the parker's state recheck but
    // before wait registration, which is the other lost-wakeup boundary.
    event(Kind::WaitEntry, key, timeout_ms);
    let queue = queue(key);
    let mut state = queue.state.lock().unwrap();
    if state.generation != observed || timeout_ms == 0 {
        return state.generation != observed;
    }
    state.waiting += 1;
    event(Kind::Registered, key, timeout_ms);
    let woke;
    if timeout_ms == u64::MAX {
        // Bound the test model so a protocol regression fails rather than
        // hanging the whole suite. Production MAX waits remain unbounded.
        let (result, timeout) = queue
            .changed
            .wait_timeout_while(state, TEST_LIMIT, |state| state.generation == observed)
            .unwrap();
        state = result;
        assert!(!timeout.timed_out(), "native parker lost a wake");
        woke = true;
    } else {
        let (result, _) = queue
            .changed
            .wait_timeout_while(state, Duration::from_millis(timeout_ms), |state| {
                state.generation == observed
            })
            .unwrap();
        state = result;
        woke = state.generation != observed;
    }
    state.waiting -= 1;
    woke
}

#[no_mangle]
extern "Rust" fn trueos_tokio_platform_wake_one(key: u64) -> bool {
    let queue = queue(key);
    let mut state = queue.state.lock().unwrap();
    state.generation = state.generation.wrapping_add(1);
    let waiting = state.waiting != 0;
    queue.changed.notify_one();
    waiting
}

fn await_event(events: &mpsc::Receiver<Event>, kind: Kind) -> Event {
    loop {
        let next = events
            .recv_timeout(TEST_LIMIT)
            .expect("parker never reached platform checkpoint");
        if next.kind == kind {
            return next;
        }
    }
}

fn assert_pending(done: &mpsc::Receiver<()>) {
    assert!(matches!(
        done.recv_timeout(Duration::from_millis(15)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
}

#[test]
fn unpark_before_park_never_enters_native_wait() {
    let parker = ThreadParker::new();
    unsafe {
        parker.prepare_park();
        let handle = parker.unpark_lock();
        assert!(!parker.timed_out());
        let (tx, rx) = mpsc::channel();
        PROBE.set(Some(Probe {
            events: tx,
            pause: None,
        }));
        parker.park();
        assert!(parker.park_until(Instant::now() - Duration::from_secs(1)));
        PROBE.set(None);
        assert!(
            rx.try_recv().is_err(),
            "an already released parker called platform wait"
        );
        handle.unpark();
    }
}

#[test]
fn wake_at_both_observe_and_registration_boundaries_is_preserved() {
    for checkpoint in [Kind::Observed, Kind::WaitEntry] {
        let parker = Arc::new(ThreadParker::new());
        unsafe {
            parker.prepare_park();
        }
        let (events_tx, events) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let worker_parker = parker.clone();
        let worker = thread::spawn(move || {
            PROBE.set(Some(Probe {
                events: events_tx,
                pause: Some((checkpoint, gate)),
            }));
            unsafe {
                worker_parker.park();
            }
            done_tx.send(()).unwrap();
        });
        await_event(&events, checkpoint);
        unsafe {
            parker.unpark_lock().unpark();
        }
        release.send(()).unwrap();
        done.recv_timeout(TEST_LIMIT)
            .expect("wake in the observe/sleep gap was lost");
        worker.join().unwrap();
    }
}

#[test]
fn native_spurious_wakes_do_not_release_the_parker() {
    let parker = Arc::new(ThreadParker::new());
    unsafe {
        parker.prepare_park();
    }
    let (events_tx, events) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let worker_parker = parker.clone();
    let worker = thread::spawn(move || {
        PROBE.set(Some(Probe {
            events: events_tx,
            pause: None,
        }));
        unsafe {
            worker_parker.park();
        }
        done_tx.send(()).unwrap();
    });
    let registered = await_event(&events, Kind::Registered);
    assert!(trueos_tokio_platform_wake_one(registered.key));
    await_event(&events, Kind::Registered);
    assert_pending(&done);
    unsafe {
        parker.unpark_lock().unpark();
    }
    done.recv_timeout(TEST_LIMIT).unwrap();
    worker.join().unwrap();
}

#[test]
fn timeout_state_is_arbitrated_after_the_queue_lock_is_reacquired() {
    let parker = ThreadParker::new();
    unsafe {
        parker.prepare_park();
        assert!(!parker.park_until(Instant::now() - Duration::from_secs(1)));
        assert!(parker.timed_out());
        // An unparker can win the queue lock after park_until returns false.
        // timed_out must then report that this was a real unpark, not timeout.
        let handle = parker.unpark_lock();
        assert!(!parker.timed_out());
        handle.unpark();
        parker.prepare_park();
        assert!(
            parker.timed_out(),
            "a new cycle inherited the previous unpark"
        );
    }
}

#[test]
fn finite_waits_round_up_and_respect_the_deadline() {
    for (duration, expected) in [
        (Duration::from_nanos(1), 1),
        (Duration::from_nanos(999_999), 1),
        (Duration::from_millis(1), 1),
        (Duration::from_nanos(1_000_001), 2),
        (Duration::MAX, u64::MAX - 1),
    ] {
        assert_eq!(thread_parker::timeout_for_test(duration), expected);
    }
    let parker = ThreadParker::new();
    let (tx, rx) = mpsc::channel();
    PROBE.set(Some(Probe {
        events: tx,
        pause: None,
    }));
    let start = Instant::now();
    let timeout = start + Duration::from_micros(500);
    unsafe {
        parker.prepare_park();
        assert!(!parker.park_until(timeout));
        assert!(parker.timed_out());
    }
    PROBE.set(None);
    assert!(
        Instant::now() >= timeout,
        "finite park returned before its deadline"
    );
    let calls: Vec<_> = rx
        .try_iter()
        .filter(|event| event.kind == Kind::WaitEntry)
        .collect();
    // A deschedule can consume a sub-millisecond deadline before the first
    // wait. Exact rounding and sentinel boundaries are checked above.
    assert!(calls
        .iter()
        .all(|event| event.timeout_ms > 0 && event.timeout_ms < u64::MAX));
    assert!(start.elapsed() < TEST_LIMIT);
}

#[test]
fn timed_park_retries_spurious_native_wakes_and_then_accepts_real_unpark() {
    let parker = Arc::new(ThreadParker::new());
    unsafe {
        parker.prepare_park();
    }
    let (events_tx, events) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let worker_parker = parker.clone();
    let worker = thread::spawn(move || {
        PROBE.set(Some(Probe {
            events: events_tx,
            pause: None,
        }));
        let unparked = unsafe { worker_parker.park_until(Instant::now() + TEST_LIMIT) };
        done_tx.send(unparked).unwrap();
    });
    for _ in 0..5 {
        let registered = await_event(&events, Kind::Registered);
        assert!(trueos_tokio_platform_wake_one(registered.key));
    }
    await_event(&events, Kind::Registered);
    assert!(
        done.try_recv().is_err(),
        "a timed park treated a native wake as an unpark token"
    );
    unsafe {
        parker.unpark_lock().unpark();
    }
    assert!(done.recv_timeout(TEST_LIMIT).unwrap());
    worker.join().unwrap();
}

#[test]
fn delayed_unpark_handle_survives_source_parker_destruction() {
    let parker = Box::new(ThreadParker::new());
    let handle = unsafe {
        parker.prepare_park();
        parker.unpark_lock()
    };
    drop(parker);
    // The handle must only contain an opaque key; it cannot touch the freed
    // thread-local parker when its actual wake is delayed past thread exit.
    unsafe {
        handle.unpark();
    }
}

#[test]
fn delayed_old_handle_at_reused_address_only_causes_a_spurious_wake() {
    let mut parker = Arc::new(ThreadParker::new());
    let old_handle = unsafe {
        parker.prepare_park();
        parker.unpark_lock()
    };
    // Replace the original object in exactly the same allocation.
    *Arc::get_mut(&mut parker).unwrap() = ThreadParker::new();
    unsafe {
        parker.prepare_park();
    }
    let (events_tx, events) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let worker_parker = parker.clone();
    let worker = thread::spawn(move || {
        PROBE.set(Some(Probe {
            events: events_tx,
            pause: None,
        }));
        unsafe {
            worker_parker.park();
        }
        done_tx.send(()).unwrap();
    });
    await_event(&events, Kind::Registered);
    unsafe {
        old_handle.unpark();
    }
    await_event(&events, Kind::Registered);
    assert_pending(&done);
    unsafe {
        parker.unpark_lock().unpark();
    }
    done.recv_timeout(TEST_LIMIT).unwrap();
    worker.join().unwrap();
}

#[test]
fn equal_opaque_keys_in_different_realms_do_not_wake_each_other() {
    let key = 0x5041_524b_5445_5354;
    let (ready_tx, ready) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let worker = thread::spawn(move || {
        SCOPE.set(23);
        let observed = trueos_tokio_platform_wait_observe(key);
        PROBE.set(Some(Probe {
            events: ready_tx,
            pause: None,
        }));
        assert!(trueos_tokio_platform_wait_after(key, observed, u64::MAX));
        done_tx.send(()).unwrap();
    });
    await_event(&ready, Kind::Registered);
    SCOPE.set(24);
    assert!(!trueos_tokio_platform_wake_one(key));
    assert_pending(&done);
    SCOPE.set(23);
    assert!(trueos_tokio_platform_wake_one(key));
    SCOPE.set(0);
    done.recv_timeout(TEST_LIMIT).unwrap();
    worker.join().unwrap();
}

#[test]
fn repeated_many_thread_handoffs_publish_data_and_reuse_parker_state() {
    const WORKERS: usize = 8;
    const CYCLES: usize = 1000;
    let parkers: Vec<_> = (0..WORKERS)
        .map(|_| Arc::new(ThreadParker::new()))
        .collect();
    let payloads: Vec<_> = (0..WORKERS)
        .map(|_| Arc::new(AtomicUsize::new(0)))
        .collect();
    let (requests_tx, requests) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let workers: Vec<_> = (0..WORKERS)
        .map(|index| {
            let parker = parkers[index].clone();
            let payload = payloads[index].clone();
            let requests = requests_tx.clone();
            let done = done_tx.clone();
            thread::spawn(move || {
                for cycle in 1..=CYCLES {
                    unsafe {
                        parker.prepare_park();
                    }
                    requests.send((index, cycle)).unwrap();
                    if cycle % 7 == 0 {
                        thread::yield_now();
                    }
                    unsafe {
                        parker.park();
                    }
                    // The payload deliberately uses relaxed accesses: release in
                    // unpark_lock and acquire in park must publish it correctly.
                    assert_eq!(payload.load(Ordering::Relaxed), cycle);
                }
                done.send(()).unwrap();
            })
        })
        .collect();
    for _ in 0..WORKERS * CYCLES {
        let (index, cycle) = requests
            .recv_timeout(TEST_LIMIT)
            .expect("repeated park lost a wake");
        payloads[index].store(cycle, Ordering::Relaxed);
        if cycle % 3 == 0 {
            thread::yield_now();
        }
        unsafe {
            parkers[index].unpark_lock().unpark();
        }
    }
    for _ in 0..WORKERS {
        done.recv_timeout(TEST_LIMIT).unwrap();
    }
    for worker in workers {
        worker.join().unwrap();
    }
}
