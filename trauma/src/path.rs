//! Platform path types used by Trauma's filesystem interface.
//!
//! Native paths are UTF8 values; desktop tests retain std filesystem paths.

#[cfg(not(target_os = "trueos"))]
pub use std::path::{Path, PathBuf};
#[cfg(target_os = "trueos")]
pub use trueos_path::{Path, PathBuf};

pub(crate) fn default_directory() -> PathBuf {
    #[cfg(target_os = "trueos")]
    {
        trueos::env::current_dir()
            .map(PathBuf::from)
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "trueos"))]
    {
        std::env::current_dir().unwrap_or_default()
    }
}
