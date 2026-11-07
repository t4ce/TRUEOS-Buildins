//! Host fixture for the existing kernel wait ABI; the C bridge under test is
//! the production Rust module, not a second copy of that bridge.
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex, OnceLock, atomic::{AtomicU64, Ordering}},
    time::{Duration, Instant},
};

#[path = "../../src/native_wait.rs"]
mod production;

#[derive(Default)]
struct Slot { generation: Mutex<u32>, changed: Condvar }
static SLOTS: OnceLock<Mutex<HashMap<u64, Arc<Slot>>>> = OnceLock::new();
static CALLS: AtomicU64 = AtomicU64::new(0);
static SPURIOUS: AtomicU64 = AtomicU64::new(0);
fn slot(key: u64) -> Arc<Slot> {
    SLOTS.get_or_init(Default::default).lock().unwrap().entry(key)
        .or_insert_with(|| Arc::new(Slot::default())).clone()
}

#[unsafe(no_mangle)]
pub extern "Rust" fn trueos_tokio_platform_wait_observe(key: u64) -> u32 {
    *slot(key).generation.lock().unwrap()
}
#[unsafe(no_mangle)]
pub extern "Rust" fn trueos_tokio_platform_wait_after(key: u64, observed: u32, ms: u64) -> bool {
    // Widen the unlock-to-park race on every call, and inject scheduling-only
    // returns regularly. The production predicate must resist both.
    std::thread::yield_now();
    if CALLS.fetch_add(1, Ordering::Relaxed) % 7 == 0 {
        SPURIOUS.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    let slot = slot(key);
    let mut generation = slot.generation.lock().unwrap();
    if ms == u64::MAX {
        while *generation == observed {
            generation = slot.changed.wait(generation).unwrap();
        }
    } else {
        let deadline = Instant::now().checked_add(Duration::from_millis(ms)).unwrap();
        while *generation == observed {
            let now = Instant::now();
            if now >= deadline { return false; }
            generation = slot.changed.wait_timeout(generation, deadline - now).unwrap().0;
        }
    }
    true
}
#[unsafe(no_mangle)]
pub extern "Rust" fn trueos_tokio_platform_wake_one(key: u64) -> bool {
    let slot = slot(key);
    let mut generation = slot.generation.lock().unwrap();
    *generation = generation.wrapping_add(1);
    slot.changed.notify_one();
    true
}
#[unsafe(no_mangle)]
pub extern "Rust" fn trueos_platform_monotonic_nanos() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}
#[unsafe(no_mangle)]
pub extern "C" fn fixture_spurious_returns() -> u64 { SPURIOUS.load(Ordering::Relaxed) }
