//! Atomic publication of an already-flushed checkpoint in the same directory.
//!
//! TRUEOS's Tokio rename currently refuses an existing destination, and its
//! private std rename implementation copies then removes. The kernel's existing
//! libc `rename` binding instead publishes one replacement namespace record.
use crate::path::Path;
use std::io;

pub(crate) async fn replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "trueos", target_os = "zkvm"))]
    {
        native::replace(from, to)
    }
    #[cfg(not(any(target_os = "trueos", target_os = "zkvm")))]
    {
        tokio::fs::rename(from, to).await
    }
}

#[cfg(any(target_os = "trueos", target_os = "zkvm", all(test, unix)))]
mod native {
    use super::{io, Path};
    use alloc::ffi::CString;
    use core::ffi::{c_char, c_int};
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        #[link_name = "rename"]
        fn rename_file(from: *const c_char, to: *const c_char) -> c_int;
    }

    pub(super) fn replace(from: &Path, to: &Path) -> io::Result<()> {
        let from = CString::new(from.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "checkpoint path contains NUL")
        })?;
        let to = CString::new(to.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "checkpoint path contains NUL")
        })?;
        // SAFETY: Both pointers address live, NUL-terminated strings for the
        // entire call. The supported kernel import has this libc signature.
        let result = unsafe { rename_file(from.as_ptr(), to.as_ptr()) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{native, Path};
    use crate::path::PathBuf;
    use alloc::format;
    use core::sync::atomic::{AtomicU64, Ordering};
    use std::{fs, io};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "trauma-checkpoint-rename-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn native_publication_replaces_an_existing_checkpoint() {
        let directory = Directory::new();
        let source = directory.0.join("pending");
        let destination = directory.0.join("checkpoint");
        fs::write(&source, b"new checkpoint").unwrap();
        fs::write(&destination, b"old checkpoint").unwrap();
        native::replace(&source, &destination).unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"new checkpoint");
        assert!(!source.exists());
    }

    #[test]
    fn failed_publication_preserves_the_existing_checkpoint() {
        let directory = Directory::new();
        let destination = directory.0.join("checkpoint");
        fs::write(&destination, b"old checkpoint").unwrap();
        assert!(native::replace(&directory.0.join("missing"), &destination).is_err());
        assert_eq!(fs::read(destination).unwrap(), b"old checkpoint");
    }

    #[test]
    fn nul_paths_are_rejected_before_entering_the_filesystem() {
        let error = native::replace(Path::new("bad\0path"), Path::new("unused")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
