//! Host fixture for the existing kernel wait ABI; the C bridge under test is
//! the production Rust module, not a second copy of that bridge.
extern crate alloc;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::HashMap,
    sync::{Arc, Condvar, Mutex, OnceLock, atomic::{AtomicBool, AtomicU64, Ordering}},
    time::{Duration, Instant},
};

#[path = "../../src/native_wait.rs"]
mod production;
#[path = "../../src/native_alloc.rs"]
mod production_alloc;
#[path = "../../src/native_time.rs"]
mod production_time;

// Real allocation failures (not merely invalid sizes) are injected before the
// concurrent wait tests. These hooks belong only to this host carrier fixture.
struct FixtureAllocator;
static FAIL_ALLOCATION: AtomicBool = AtomicBool::new(false);
static LIVE_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
#[global_allocator]
static ALLOCATOR: FixtureAllocator = FixtureAllocator;
unsafe impl GlobalAlloc for FixtureAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if FAIL_ALLOCATION.swap(false, Ordering::Relaxed) { return core::ptr::null_mut(); }
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() { LIVE_ALLOCATIONS.fetch_add(1, Ordering::Relaxed); }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if FAIL_ALLOCATION.swap(false, Ordering::Relaxed) { return core::ptr::null_mut(); }
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() { LIVE_ALLOCATIONS.fetch_add(1, Ordering::Relaxed); }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_ALLOCATIONS.fetch_sub(1, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if FAIL_ALLOCATION.swap(false, Ordering::Relaxed) { return core::ptr::null_mut(); }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn fixture_fail_next_allocation() { FAIL_ALLOCATION.store(true, Ordering::Relaxed); }
#[unsafe(no_mangle)]
pub extern "C" fn fixture_live_allocations() -> u64 { LIVE_ALLOCATIONS.load(Ordering::Relaxed) }

#[unsafe(no_mangle)]
pub extern "C" fn trueos_time_unix_nanos() -> u64 { 1_700_000_000_123_000_000 }
#[unsafe(no_mangle)]
pub extern "C" fn trueos_time_timezone_offset_seconds(seconds: i64) -> i32 {
    // A deterministic offset transition lets JS exercise timestamp-dependent
    // civil time without depending on the host machine's configured zone.
    if seconds < 1_700_000_000 { 3600 } else { 7200 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sys_rand(words: *mut u32, count: usize) {
    for i in 0..count { unsafe { words.add(i).write(0x1234_abcd ^ i as u32) }; }
}

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
