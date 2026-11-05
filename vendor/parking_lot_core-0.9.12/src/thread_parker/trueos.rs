// Copyright 2016 Amanieu d'Antras
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

extern "Rust" {
    fn trueos_tokio_platform_wait_observe(key: u64) -> u32;
    fn trueos_tokio_platform_wait_after(key: u64, observed: u32, timeout_ms: u64) -> bool;
    fn trueos_tokio_platform_wake_one(key: u64) -> bool;
}

const PARKED: u8 = 1;
const UNPARKED: u8 = 0;

// The flag owns the park/unpark decision. Native wait generations only arrange
// sleeping and waking; a spurious wake must never grant an unpark token.
pub struct ThreadParker {
    state: AtomicU8,
}

impl super::ThreadParkerT for ThreadParker {
    type UnparkHandle = UnparkHandle;

    const IS_CHEAP_TO_CONSTRUCT: bool = true;

    #[inline]
    fn new() -> ThreadParker {
        ThreadParker {
            state: AtomicU8::new(UNPARKED),
        }
    }

    #[inline]
    unsafe fn prepare_park(&self) {
        self.state.store(PARKED, Ordering::Relaxed);
    }

    #[inline]
    unsafe fn timed_out(&self) -> bool {
        self.state.load(Ordering::Acquire) == PARKED
    }

    #[inline]
    unsafe fn park(&self) {
        let key = self.key();
        while self.state.load(Ordering::Acquire) == PARKED {
            let observed = trueos_tokio_platform_wait_observe(key);
            // Observe before checking the flag: an unpark between this check
            // and wait_after changes the generation, so it cannot be missed.
            if self.state.load(Ordering::Acquire) == UNPARKED {
                return;
            }
            trueos_tokio_platform_wait_after(key, observed, u64::MAX);
        }
    }

    #[inline]
    unsafe fn park_until(&self, timeout: Instant) -> bool {
        let key = self.key();
        while self.state.load(Ordering::Acquire) == PARKED {
            let observed = trueos_tokio_platform_wait_observe(key);
            if self.state.load(Ordering::Acquire) == UNPARKED {
                return true;
            }
            let now = Instant::now();
            if timeout <= now {
                // parking_lot_core checks timed_out again while holding its
                // queue lock, settling an unpark racing with this deadline.
                return false;
            }
            trueos_tokio_platform_wait_after(key, observed, timeout_millis(timeout - now));
        }
        true
    }

    #[inline]
    unsafe fn unpark_lock(&self) -> UnparkHandle {
        let key = self.key();
        // Publish the token and everything preceding the unpark while the
        // parking_lot queue is still locked, just like the Linux futex backend.
        self.state.store(UNPARKED, Ordering::Release);
        UnparkHandle { key }
    }
}

impl ThreadParker {
    #[inline]
    fn key(&self) -> u64 {
        // TRUEOS scopes keys to the caller's VM; this is an opaque identity,
        // never a pointer for the platform to dereference.
        &self.state as *const AtomicU8 as usize as u64
    }
}

// TRUEOS uses milliseconds, zero for a probe, and MAX for infinity. Round up
// finite waits, reserving both sentinel values, and recheck Instant afterwards.
#[inline]
fn timeout_millis(duration: Duration) -> u64 {
    let millis = duration.as_nanos().saturating_add(999_999) / 1_000_000;
    millis.min((u64::MAX - 1) as u128) as u64
}

pub struct UnparkHandle {
    key: u64,
}

impl super::UnparkHandleT for UnparkHandle {
    #[inline]
    unsafe fn unpark(self) {
        // The owner can finish, destroy its ThreadData, or reuse its parker
        // after unpark_lock. Retain only the numeric key: no access to its
        // allocation is permitted here. A late wake at a reused address is
        // harmless because the new park checks its own flag after every wake.
        trueos_tokio_platform_wake_one(self.key);
    }
}

#[inline]
pub fn thread_yield() {
    std::thread::yield_now();
}
