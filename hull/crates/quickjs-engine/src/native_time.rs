//! Scalar native time/entropy boundary for the pinned C engine. Civil-time
//! rules remain owned by TRUEOS; no timeval, tm, errno, or C timezone state is
//! constructed merely to ask for an offset.
unsafe extern "C" {
    fn trueos_time_unix_nanos() -> u64;
    fn trueos_time_timezone_offset_seconds(utc_seconds: i64) -> i32;
    fn sys_rand(words: *mut u32, count: usize);
}

#[unsafe(no_mangle)]
pub extern "C" fn hull_quickjs_native_random_seed() -> u64 {
    let mut words = [0u32; 2];
    unsafe { sys_rand(words.as_mut_ptr(), words.len()) };
    u64::from(words[0]) | (u64::from(words[1]) << 32)
}

#[unsafe(no_mangle)]
pub extern "C" fn hull_quickjs_native_unix_millis() -> i64 {
    (unsafe { trueos_time_unix_nanos() } / 1_000_000) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn hull_quickjs_native_timezone_offset_seconds(utc_seconds: i64) -> i32 {
    unsafe { trueos_time_timezone_offset_seconds(utc_seconds) }
}
