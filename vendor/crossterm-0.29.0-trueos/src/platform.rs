//! Runtime primitives selected explicitly for the terminal backend.
//!
//! TRUEOS uses the shared native runtime; desktop clients keep their existing
//! parking_lot and std clock contracts.

#[cfg(not(target_os = "trueos"))]
pub(crate) use parking_lot::{Mutex, MutexGuard, Once};
#[cfg(not(target_os = "trueos"))]
pub(crate) use std::time::Instant;

#[cfg(target_os = "trueos")]
pub(crate) use trueos_runtime::{sync::{MutexGuard, Once}, time::Instant};

#[cfg(target_os = "trueos")]
pub(crate) struct Mutex<T>(trueos_runtime::sync::Mutex<T>);
#[cfg(target_os = "trueos")]
impl<T> Mutex<T> {
    pub(crate) const fn new(value: T) -> Self { Self(trueos_runtime::sync::Mutex::new(value)) }
    pub(crate) fn lock(&self) -> MutexGuard<'_, T> { self.0.lock().unwrap() }
    pub(crate) fn try_lock_for(&self, duration: core::time::Duration) -> Option<MutexGuard<'_, T>> {
        self.0.try_lock_for(duration)
    }
}
