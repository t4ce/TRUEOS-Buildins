#[allow(unused_imports)]
use crate::alloc_prelude::*;
#[cfg(feature = "event-stream")]
pub(crate) mod waker;

#[cfg(feature = "events")]
pub(crate) mod parse;
