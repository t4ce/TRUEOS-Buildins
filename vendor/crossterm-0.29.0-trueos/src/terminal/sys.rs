//! This module provides platform related functions.

#[cfg(all(unix, not(target_os = "trueos")))]
#[cfg(feature = "events")]
pub use self::unix::supports_keyboard_enhancement;
#[cfg(all(unix, not(target_os = "trueos")))]
pub(crate) use self::unix::{
    disable_raw_mode, enable_raw_mode, is_raw_mode_enabled, size, window_size,
};
#[cfg(all(unix, not(target_os = "trueos")))]
pub mod file_descriptor;
#[cfg(all(unix, not(target_os = "trueos")))]
mod unix;

#[cfg(target_os = "trueos")]
#[cfg(feature = "events")]
pub use self::trueos::supports_keyboard_enhancement;
#[cfg(target_os = "trueos")]
pub(crate) use self::trueos::{
    disable_raw_mode, enable_raw_mode, is_raw_mode_enabled, size, window_size,
};
#[cfg(target_os = "trueos")]
mod trueos;
