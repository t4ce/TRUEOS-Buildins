//! Thread-safe task notification primitives.

#[allow(unused_imports)]
use crate::alloc_prelude::*;
mod atomic_waker;
pub(crate) use self::atomic_waker::AtomicWaker;
