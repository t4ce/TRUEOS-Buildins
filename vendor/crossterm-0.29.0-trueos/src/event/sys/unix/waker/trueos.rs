#[allow(unused_imports)]
use crate::alloc_prelude::*;
use crate::io;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};

/// The native source has one queued wake token. Publishing it before changing
/// the VM-local I/O generation closes both sides of the wait-registration race.
#[derive(Clone, Debug)]
pub(crate) struct Waker(Arc<AtomicBool>);

impl Waker {
    pub(crate) fn new() -> Self { Self(Arc::new(AtomicBool::new(false))) }

    pub(crate) fn take(&self) -> bool { self.0.swap(false, Ordering::AcqRel) }

    pub(crate) fn wake(&self) -> io::Result<()> {
        self.0.store(true, Ordering::Release);
        crate::trueos::wake()
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::alloc_prelude::*;
    use super::*;

    #[test]
    fn cloned_waker_publishes_one_token_before_native_generation_wake() {
        let _serial = crate::trueos::test_support::SERIAL.lock().unwrap();
        let waker = Waker::new();
        let cloned = waker.clone();
        let observed = crate::trueos::observe();
        cloned.wake().unwrap();
        assert!(crate::trueos::observe() != observed);
        assert!(waker.take());
        assert!(!cloned.take());
        waker.wake().unwrap();
        waker.wake().unwrap();
        assert!(cloned.take());
        assert!(!waker.take());
    }
}
