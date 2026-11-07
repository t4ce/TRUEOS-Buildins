#[allow(unused_imports)]
use crate::alloc_prelude::*;
#[cfg(all(any(unix, target_os = "trueos"), feature = "event-stream"))]
pub(crate) use unix::waker::Waker;
#[cfg(any(unix, target_os = "trueos"))]
pub(crate) mod unix;
