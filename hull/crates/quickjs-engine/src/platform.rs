// Desktop fixtures keep unwinding and the host clock. The native engine runs
// with panic=abort, so catching a Rust panic never provided recovery there.
#[cfg(not(target_os = "trueos"))]
pub(crate) fn invoke_host<F: FnOnce() -> T, T>(call: F) -> Result<T, ()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).map_err(|_| ())
}

#[cfg(target_os = "trueos")]
pub(crate) fn invoke_host<F: FnOnce() -> T, T>(call: F) -> Result<T, ()> {
    Ok(call())
}

#[cfg(not(target_os = "trueos"))]
pub(crate) use std::time::Instant;

#[cfg(target_os = "trueos")]
pub(crate) use native::Instant;

#[cfg(any(target_os = "trueos", test))]
mod native {
    use core::{ops::Add, time::Duration};

    unsafe extern "Rust" {
        fn trueos_platform_monotonic_nanos() -> u64;
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub(crate) struct Instant(u64);

    impl Instant {
        pub(crate) fn now() -> Self {
            Self(unsafe { trueos_platform_monotonic_nanos() })
        }

        pub(crate) fn checked_add(self, duration: Duration) -> Option<Self> {
            self.0
                .checked_add(u64::try_from(duration.as_nanos()).ok()?)
                .map(Self)
        }

        pub(crate) fn elapsed(self) -> Duration {
            Duration::from_nanos(Self::now().0.saturating_sub(self.0))
        }
    }

    impl Add<Duration> for Instant {
        type Output = Self;
        fn add(self, duration: Duration) -> Self {
            self.checked_add(duration)
                .expect("QuickJS monotonic deadline overflow")
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use core::sync::atomic::{AtomicU64, Ordering};
        static NOW: AtomicU64 = AtomicU64::new(0);
        #[unsafe(no_mangle)]
        extern "Rust" fn trueos_platform_monotonic_nanos() -> u64 {
            NOW.load(Ordering::Relaxed)
        }

        #[test]
        fn native_deadlines_use_nanos_checked_bounds_and_saturating_elapsed() {
            NOW.store(1_000_000, Ordering::Relaxed);
            let started = Instant::now();
            let deadline = started + Duration::from_millis(125);
            NOW.store(125_999_999, Ordering::Relaxed);
            assert!(Instant::now() < deadline);
            NOW.store(126_000_000, Ordering::Relaxed);
            assert_eq!(Instant::now(), deadline);
            assert_eq!(started.elapsed(), Duration::from_millis(125));
            NOW.store(0, Ordering::Relaxed);
            assert_eq!(started.elapsed(), Duration::ZERO);
            assert_eq!(started.checked_add(Duration::MAX), None);
            assert_eq!(Instant(u64::MAX).checked_add(Duration::from_nanos(1)), None);
        }
    }
}
