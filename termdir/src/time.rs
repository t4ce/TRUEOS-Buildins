pub use core::time::Duration;
#[cfg(not(target_os = "trueos"))]
pub use std::time::Instant;
#[cfg(target_os = "trueos")]
pub use trueos_runtime::time::Instant;
