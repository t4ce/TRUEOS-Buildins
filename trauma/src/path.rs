//! Platform path types used by Trauma's filesystem interface.
//!
//! TRUEOS builds use the platform API's path facade. That API currently
//! reexports the standard-library types, preserving compatibility with Tokio
//! filesystem operations; this is not a separate `no_std` path implementation.
//! Host builds use the standard-library types directly.

#[cfg(not(any(target_os = "trueos", target_os = "zkvm")))]
pub use std::path::{Path, PathBuf};
#[cfg(any(target_os = "trueos", target_os = "zkvm"))]
pub use trueos::platform::path::{Path, PathBuf};
