//! Engine-local C bridge to TRUEOS generation waits. No C pointer registry,
//! pthread object, or allocation is needed for QuickJS mutex/condition state.
unsafe extern "Rust" {
    fn trueos_tokio_platform_wait_observe(key: u64) -> u32;
    fn trueos_tokio_platform_wait_after(key: u64, observed: u32, timeout_ms: u64) -> bool;
    fn trueos_tokio_platform_wake_one(key: u64) -> bool;
    fn trueos_platform_monotonic_nanos() -> u64;
}

#[unsafe(no_mangle)]
pub extern "C" fn hull_quickjs_native_wait_observe(key: u64) -> u32 {
    unsafe { trueos_tokio_platform_wait_observe(key) }
}

#[unsafe(no_mangle)]
pub extern "C" fn hull_quickjs_native_wait_after(key: u64, observed: u32, timeout_ms: u64) {
    // Readiness is deliberately rechecked in C; a scheduling wake is not a
    // condition-variable notification, and timeouts use the monotonic clock.
    unsafe { trueos_tokio_platform_wait_after(key, observed, timeout_ms); }
}

#[unsafe(no_mangle)]
pub extern "C" fn hull_quickjs_native_wake_one(key: u64) {
    unsafe { trueos_tokio_platform_wake_one(key); }
}

#[unsafe(no_mangle)]
pub extern "C" fn hull_quickjs_native_monotonic_nanos() -> u64 {
    unsafe { trueos_platform_monotonic_nanos() }
}
