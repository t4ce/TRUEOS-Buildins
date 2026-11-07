//! Services needed by the platform-independent API. Data and allocation types
//! stay in core/alloc; TRUEOS uses its existing native runtime vocabulary.

#[cfg(all(feature = "trueos-native", target_os = "trueos"))]
pub(crate) use trueos_runtime::{sync::Mutex, time::Instant};
#[cfg(all(feature = "trueos-native", target_os = "trueos"))]
pub(crate) use trueos_io as io;
#[cfg(all(feature = "trueos-native", target_os = "trueos"))]
pub(crate) use trueos_path::{Path, PathBuf};

#[cfg(not(all(feature = "trueos-native", target_os = "trueos")))]
pub(crate) use std::{io, path::{Path, PathBuf}, sync::Mutex, time::Instant};
