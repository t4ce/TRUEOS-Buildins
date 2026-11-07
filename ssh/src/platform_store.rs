use crate::storage::{Snapshot, Store, StoreError};
use alloc::{format, string::ToString};
use zeroize::Zeroizing;
pub struct NativeStore;
const MAX_CONFIG_BYTES: u64 = 16 * 1024 * 1024;
#[cfg(target_os = "trueos")]
impl Store for NativeStore {
    async fn snapshot(&mut self, path: &str) -> core::result::Result<Option<Snapshot>, StoreError> {
        self.read_snapshot(path, MAX_CONFIG_BYTES).await
    }
    async fn identity_snapshot(
        &mut self,
        path: &str,
    ) -> core::result::Result<Option<Snapshot>, StoreError> {
        self.read_snapshot(path, 1024 * 1024).await
    }
    async fn ensure_parent(&mut self, path: &str) -> core::result::Result<(), StoreError> {
        if let Some((parent, _)) = path.rsplit_once('/').filter(|(p, _)| !p.is_empty()) {
            trueos::async_fs::create_dir_all(parent.as_bytes())
                .await
                .map_err(|c| status("create SSH directory", c))?
        }
        Ok(())
    }
    async fn publish(
        &mut self,
        path: &str,
        bytes: &[u8],
        expected: Option<[u8; 32]>,
    ) -> core::result::Result<(), StoreError> {
        use trueos::async_fs as fs;
        fs::write_file_typed_checked(
            path.as_bytes(),
            bytes,
            fs::ContentTypeId::UTF8_TEXT,
            expected.is_none(),
            expected,
        )
        .await
        .map_err(|c| status("publish SSH configuration", c))
    }
}
#[cfg(target_os = "trueos")]
impl NativeStore {
    async fn read_snapshot(
        &mut self,
        path: &str,
        limit: u64,
    ) -> core::result::Result<Option<Snapshot>, StoreError> {
        use trueos::async_fs as fs;
        for _ in 0..3 {
            let before = match fs::metadata_v2(path.as_bytes()).await {
                Ok(m) => m,
                Err(fs::ERR_NOT_FOUND) => return Ok(None),
                Err(c) => return Err(status("read SSH metadata", c)),
            };
            if !before.is_file() || before.len > limit {
                return Err(StoreError::Other(
                    "ssh: configuration is not a file or exceeds its size limit".into(),
                ));
            }
            let bytes = Zeroizing::new(
                fs::read_file(path.as_bytes())
                    .await
                    .map_err(|c| status("read SSH configuration", c))?,
            );
            let after = fs::metadata_v2(path.as_bytes())
                .await
                .map_err(|c| status("read SSH metadata", c))?;
            if before == after && bytes.len() as u64 == before.len {
                return Ok(Some(Snapshot { bytes }));
            }
        }
        Err(StoreError::Conflict)
    }
}
#[cfg(target_os = "trueos")]
fn status(context: &str, code: i32) -> StoreError {
    if matches!(
        code,
        trueos::async_fs::ERR_ALREADY_EXISTS | trueos::async_fs::ERR_WRITE_CONFLICT
    ) {
        StoreError::Conflict
    } else {
        StoreError::Other(format!("ssh: {context} failed ({code})"))
    }
}

#[cfg(not(target_os = "trueos"))]
mod host {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        io::{Read, Write},
        sync::{
            Mutex,
            atomic::{AtomicU64, Ordering},
        },
    };
    static PUBLICATION: Mutex<()> = Mutex::new(());
    static TEMP: AtomicU64 = AtomicU64::new(0);
    fn error(e: std::io::Error) -> StoreError {
        StoreError::Other(e.to_string())
    }
    impl Store for NativeStore {
        async fn snapshot(
            &mut self,
            path: &str,
        ) -> core::result::Result<Option<Snapshot>, StoreError> {
            let file = match fs::File::open(path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(error(e)),
            };
            let metadata = file.metadata().map_err(error)?;
            if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
                return Err(StoreError::Other(
                    "ssh: configuration is not a file or exceeds 16MiB".into(),
                ));
            }
            let mut bytes = Zeroizing::new(alloc::vec::Vec::new());
            file.take(MAX_CONFIG_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(error)?;
            if bytes.len() as u64 > MAX_CONFIG_BYTES {
                return Err(StoreError::Other("ssh: configuration exceeds 16MiB".into()));
            }
            Ok(Some(Snapshot { bytes }))
        }
        async fn ensure_parent(&mut self, path: &str) -> core::result::Result<(), StoreError> {
            if let Some(parent) = std::path::Path::new(path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
            {
                fs::create_dir_all(parent).map_err(error)?
            }
            Ok(())
        }
        async fn publish(
            &mut self,
            path: &str,
            bytes: &[u8],
            expected: Option<[u8; 32]>,
        ) -> core::result::Result<(), StoreError> {
            let _guard = PUBLICATION.lock().unwrap();
            match (fs::read(path).map(Zeroizing::new), expected) {
                (Ok(old), Some(digest))
                    if <[u8; 32]>::from(Sha256::digest(old.as_slice())) == digest => {}
                (Err(e), None) if e.kind() == std::io::ErrorKind::NotFound => {}
                (Err(e), _) if e.kind() != std::io::ErrorKind::NotFound => return Err(error(e)),
                _ => return Err(StoreError::Conflict),
            }
            let temporary = format!("{path}.ssh-tmp-{}", TEMP.fetch_add(1, Ordering::Relaxed));
            let result = (|| {
                let mut f = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)
                    .map_err(error)?;
                f.write_all(bytes).map_err(error)?;
                f.flush().map_err(error)?;
                drop(f);
                if expected.is_some() {
                    fs::rename(&temporary, path).map_err(error)?;
                } else {
                    fs::hard_link(&temporary, path).map_err(|e| {
                        if e.kind() == std::io::ErrorKind::AlreadyExists {
                            StoreError::Conflict
                        } else {
                            error(e)
                        }
                    })?;
                }
                Ok(())
            })();
            let _ = fs::remove_file(temporary);
            result
        }
    }
}
