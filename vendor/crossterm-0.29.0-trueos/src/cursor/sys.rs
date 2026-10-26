//! This module provides platform related functions.

#[cfg(unix)]
#[cfg(feature = "events")]
pub use self::unix::position;
#[cfg(unix)]
#[cfg(feature = "events")]
pub(crate) mod unix;
