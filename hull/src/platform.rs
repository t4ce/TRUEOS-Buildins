//! The browser's monotonic clock and worker-lane lock.
//!
//! The UI and JavaScript lane exchange only owned data. Native locks wait on
//! execution-context generations rather than importing hosted thread services.
pub use core::time::Duration;

#[cfg(target_os = "trueos")]
pub use trueos_runtime::{sync::Mutex, time::Instant};

#[cfg(not(target_os = "trueos"))]
pub use std::{sync::Mutex, time::Instant};
