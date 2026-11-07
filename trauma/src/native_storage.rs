//! The previous native Tokio bridge buffered complete files too. Keep that
//! memory contract, while publishing only explicitly and tracking committed bytes.
use crate::{
    body::Sink,
    path::{Path, PathBuf},
};
use alloc::{format, string::String, vec::Vec};
use sha2::{Digest, Sha256};
use trueos::async_fs::{self as fs, ContentTypeId, MetadataV2};

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const SNAPSHOT_ATTEMPTS: usize = 3;

fn error(context: &str, code: i32) -> String {
    format!("{context} failed ({code})")
}

struct Snapshot {
    metadata: MetadataV2,
    content_type: ContentTypeId,
    bytes: Vec<u8>,
}

async fn snapshot(path: &Path, limit: u64) -> Result<Option<Snapshot>, String> {
    for _ in 0..SNAPSHOT_ATTEMPTS {
        let before = match fs::metadata_v2(path.as_str().as_bytes()).await {
            Ok(metadata) => metadata,
            Err(fs::ERR_NOT_FOUND) => return Ok(None),
            Err(code) => return Err(error("download metadata", code)),
        };
        if !before.is_file() {
            return Err("download destination is not a file".into());
        }
        if before.len > limit {
            return Err("download exceeds the native file buffer limit".into());
        }
        let typed = fs::typed_metadata(path.as_str().as_bytes())
            .await
            .map_err(|code| error("download type", code))?;
        let bytes = fs::read_file(path.as_str().as_bytes())
            .await
            .map_err(|code| error("download read", code))?;
        let after = fs::metadata_v2(path.as_str().as_bytes())
            .await
            .map_err(|code| error("download metadata", code))?;
        if before == after
            && typed.kind == before.kind
            && typed.len == before.len
            && bytes.len() as u64 == before.len
        {
            return Ok(Some(Snapshot {
                metadata: before,
                content_type: typed.content_type,
                bytes,
            }));
        }
    }
    Err("download changed while taking its committed snapshot".into())
}

pub(crate) async fn file_len(path: &Path) -> Result<Option<u64>, String> {
    match fs::metadata_v2(path.as_str().as_bytes()).await {
        Ok(metadata) if metadata.is_file() => Ok(Some(metadata.len)),
        Ok(_) => Err("download destination is not a file".into()),
        Err(fs::ERR_NOT_FOUND) => Ok(None),
        Err(code) => Err(error("download metadata", code)),
    }
}

pub(crate) async fn create_dir_all(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path.as_str().as_bytes())
        .await
        .map_err(|code| error("download directory", code))
}

pub(crate) struct File {
    path: PathBuf,
    bytes: Vec<u8>,
    durable_len: u64,
    expected: [u8; 32],
    dirty: bool,
}

pub(crate) async fn open(path: &Path, offset: u64) -> Result<File, String> {
    let saved = snapshot(path, MAX_FILE_BYTES).await?;
    if offset > 0 {
        let saved = saved.ok_or_else(|| String::from("resume destination disappeared"))?;
        if saved.metadata.len != offset {
            return Err("resume destination length changed".into());
        }
        let expected = Sha256::digest(&saved.bytes).into();
        return Ok(File {
            path: path.to_path_buf(),
            bytes: saved.bytes,
            durable_len: offset,
            expected,
            dirty: false,
        });
    }
    // Publish the validated restart BEFORE a new representation checkpoint.
    // A crash must never attach the new ETag to the previous on-disk bytes.
    match saved {
        Some(saved) => {
            let expected = Sha256::digest(&saved.bytes).into();
            if saved.content_type != ContentTypeId::BLOB {
                // The kernel atomically checks both content and the current FFA
                // key, so a concurrent protected record cannot be removed/reset.
                fs::reset_blob_checked(path.as_str().as_bytes(), expected)
                    .await
                    .map_err(|code| {
                        error("reset typed download to Blob; old record retained", code)
                    })?;
            } else {
                fs::write_file_typed_checked(
                    path.as_str().as_bytes(),
                    &[],
                    ContentTypeId::BLOB,
                    false,
                    Some(expected),
                )
                .await
                .map_err(|code| error("truncate download", code))?;
            }
        }
        None => {
            fs::write_file_typed_checked(
                path.as_str().as_bytes(),
                &[],
                ContentTypeId::BLOB,
                true,
                None,
            )
            .await
            .map_err(|code| error("create download", code))?;
        }
    }
    Ok(File {
        path: path.to_path_buf(),
        bytes: Vec::new(),
        durable_len: 0,
        expected: Sha256::digest([]).into(),
        dirty: false,
    })
}

impl Sink for File {
    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), String> {
        let length = (self.bytes.len() as u64)
            .checked_add(bytes.len() as u64)
            .filter(|length| *length <= MAX_FILE_BYTES)
            .ok_or_else(|| String::from("download exceeds native 4GiB file limit"))?;
        usize::try_from(length).map_err(|_| "download exceeds addressable buffer size")?;
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|_| "cannot allocate download buffer; accepted prefix retained")?;
        self.bytes.extend_from_slice(bytes);
        self.dirty |= !bytes.is_empty();
        Ok(())
    }
    async fn flush(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        fs::write_file_typed_checked(
            self.path.as_str().as_bytes(),
            &self.bytes,
            ContentTypeId::BLOB,
            false,
            Some(self.expected),
        )
        .await
        .map_err(|code| error("commit download prefix", code))?;
        self.durable_len = self.bytes.len() as u64;
        self.expected = Sha256::digest(&self.bytes).into();
        self.dirty = false;
        Ok(())
    }
    async fn committed_len(&mut self) -> Result<u64, String> {
        Ok(self.durable_len)
    }
}

pub(crate) async fn read_limited(path: &Path, max: u64) -> Result<Option<Vec<u8>>, String> {
    if file_len(path).await?.is_some_and(|length| length > max) {
        return Ok(None);
    }
    Ok(snapshot(path, max).await?.map(|saved| saved.bytes))
}

pub(crate) async fn save_checkpoint(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let saved = snapshot(path, 16 * 1024).await?;
    let (create_new, expected) = match saved {
        Some(saved) => (false, Some(Sha256::digest(&saved.bytes).into())),
        None => (true, None),
    };
    fs::write_file_typed_checked(
        path.as_str().as_bytes(),
        bytes,
        ContentTypeId::UTF8_TEXT,
        create_new,
        expected,
    )
    .await
    .map_err(|code| error("commit download checkpoint", code))
}

pub(crate) async fn remove_file_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path.as_str().as_bytes()).await {
        Ok(()) | Err(fs::ERR_NOT_FOUND) => Ok(()),
        Err(code) => Err(error("remove download checkpoint", code)),
    }
}
