//! This module provides platform related functions.

#[allow(unused_imports)]
use crate::alloc_prelude::*;
#[cfg(any(unix, target_os = "trueos"))]
#[cfg(feature = "events")]
pub use self::unix::position;
#[cfg(any(unix, target_os = "trueos"))]
#[cfg(feature = "events")]
pub(crate) mod unix;
