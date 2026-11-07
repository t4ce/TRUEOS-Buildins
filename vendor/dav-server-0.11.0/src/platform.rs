//! Types supplied by the execution platform rather than WebDAV itself.

#[cfg(all(feature = "trueos-native", target_os = "trueos"))]
pub use trueos_io as io;
#[cfg(not(all(feature = "trueos-native", target_os = "trueos")))]
pub use std::io;

#[cfg(all(feature = "trueos-native", target_os = "trueos"))]
pub use trueos_path::{Path, PathBuf};
#[cfg(not(all(feature = "trueos-native", target_os = "trueos")))]
pub use std::path::{Path, PathBuf};

pub mod collections {
    #[cfg(all(feature = "trueos-native", target_os = "trueos"))]
    pub use hashbrown::HashMap;
    #[cfg(not(all(feature = "trueos-native", target_os = "trueos")))]
    pub use std::collections::HashMap;
}

pub mod time {
    pub use core::time::Duration;
    #[cfg(all(feature = "trueos-native", target_os = "trueos"))]
    pub use httpdate::time::{SystemTime, UNIX_EPOCH};
    #[cfg(not(all(feature = "trueos-native", target_os = "trueos")))]
    pub use std::time::{SystemTime, UNIX_EPOCH};
}

pub mod sync {
    pub use alloc::sync::Arc;
    #[cfg(all(feature = "trueos-native", target_os = "trueos"))]
    pub use trueos_runtime::sync::Mutex;
    #[cfg(not(all(feature = "trueos-native", target_os = "trueos")))]
    pub use std::sync::Mutex;
}

#[cfg(all(feature = "trueos-native", target_os = "trueos"))]
pub use trueos_runtime::time::Instant;
#[cfg(not(all(feature = "trueos-native", target_os = "trueos")))]
pub use std::time::Instant;

#[cfg(all(feature = "trueos-native", target_os = "trueos"))]
pub fn new_lock_id() -> uuid::Uuid {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&trueos_runtime::random_seed().to_ne_bytes());
    bytes[8..].copy_from_slice(&trueos_runtime::random_seed().to_ne_bytes());
    uuid::Builder::from_random_bytes(bytes).into_uuid()
}
#[cfg(not(all(feature = "trueos-native", target_os = "trueos")))]
pub fn new_lock_id() -> uuid::Uuid { uuid::Uuid::new_v4() }
