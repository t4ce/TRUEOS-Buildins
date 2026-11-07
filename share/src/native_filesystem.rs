//! Native WebDAV storage: immutable read snapshots and explicit checked writes.
//!
//! A handle never publishes from Drop. DAV calls flush only after validating the
//! complete request body, so a failed or cancelled upload leaves the old record.
use alloc::{boxed::Box, vec::Vec};
use bytes::{Buf, Bytes};
use dav_server::davpath::DavPath;
use dav_server::fs::{
    DavDirEntry, DavFile, DavFileSystem, DavMetaData, FsError, FsFuture, FsResult, FsStream,
    OpenOptions, ReadDirMeta, SeekFrom, SystemTime,
};
use futures_util::stream;
use sha2::{Digest, Sha256};
use trueos::async_fs::{self as fs, ContentTypeId, MetadataV2, NodeKind};
use trueos_path::{Path, PathBuf};
use trueos_runtime::io;

pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const SNAPSHOT_ATTEMPTS: usize = 3;

fn error(code: i32) -> FsError {
    #[cfg(target_os = "trueos")]
    if !matches!(
        code,
        -1 | -3 | -4 | -5 | -6 | -7 | -8 | -9 | -15 | -16 | -17
    ) {
        trueos::vsys::log_errorf(format_args!("share: native filesystem failed ({code})\n"));
    }
    match code {
        -1 | -4 | -6 | -15 | -16 => FsError::Forbidden,
        -3 | -5 => FsError::InsufficientStorage,
        -7 => FsError::TooLarge,
        -8 => FsError::NotFound,
        -9 => FsError::Exists,
        -17 => FsError::PreconditionFailed,
        _ => FsError::GeneralFailure,
    }
}

#[derive(Clone, Debug)]
struct ShareMetadata {
    record: MetadataV2,
    content_type: ContentTypeId,
}

impl DavMetaData for ShareMetadata {
    fn len(&self) -> u64 {
        self.record.len
    }
    fn is_dir(&self) -> bool {
        self.record.is_dir()
    }
    fn modified(&self) -> FsResult<SystemTime> {
        Err(FsError::NotImplemented)
    }
    fn content_type(&self) -> Option<&str> {
        if self.is_dir() {
            return None;
        }
        Some(
            fs::content_type_info(self.content_type)
                .map_or("application/octet-stream", |info| info.mime_type),
        )
    }
}

/// Bind bytes and content type to the same committed native record revision.
/// A pathname replacement during any request causes a retry, never a mixed view.
async fn snapshot(path: &Path, with_bytes: bool) -> FsResult<(ShareMetadata, Vec<u8>)> {
    for _ in 0..SNAPSHOT_ATTEMPTS {
        let before = fs::metadata_v2(path.as_str().as_bytes())
            .await
            .map_err(error)?;
        if with_bytes && before.is_file() && before.len > MAX_FILE_BYTES {
            return Err(FsError::TooLarge);
        }
        let typed = match fs::typed_metadata(path.as_str().as_bytes()).await {
            Ok(typed) => typed,
            Err(fs::ERR_NOT_FOUND) => continue,
            Err(code) => return Err(error(code)),
        };
        let bytes = if with_bytes && before.is_file() {
            match fs::read_file(path.as_str().as_bytes()).await {
                Ok(bytes) => bytes,
                Err(fs::ERR_NOT_FOUND) => continue,
                Err(code) => return Err(error(code)),
            }
        } else {
            Vec::new()
        };
        let after = match fs::metadata_v2(path.as_str().as_bytes()).await {
            Ok(after) => after,
            Err(fs::ERR_NOT_FOUND) => continue,
            Err(code) => return Err(error(code)),
        };
        if before == after
            && typed.kind == before.kind
            && typed.len == before.len
            && (!with_bytes || !before.is_file() || bytes.len() as u64 == before.len)
        {
            return Ok((
                ShareMetadata {
                    record: before,
                    content_type: typed.content_type,
                },
                bytes,
            ));
        }
    }
    Err(FsError::PreconditionFailed)
}

#[derive(Clone)]
pub struct ShareFs {
    root: PathBuf,
}

impl ShareFs {
    pub async fn new(root: &Path) -> io::Result<Self> {
        fs::create_dir_all(root.as_str().as_bytes())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "cannot create share root"))?;
        let metadata = fs::metadata_v2(root.as_str().as_bytes())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "cannot inspect share root"))?;
        if !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "share root is not a directory",
            ));
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    async fn resolve(&self, path: &DavPath, allow_missing: bool) -> FsResult<PathBuf> {
        let text = core::str::from_utf8(path.as_bytes()).map_err(|_| FsError::Forbidden)?;
        let text = text.trim_matches('/');
        let mut result = self.root.clone();
        if text.is_empty() {
            return Ok(result);
        }
        let mut components = text.split('/').peekable();
        while let Some(name) = components.next() {
            if name.is_empty() || name == "." || name == ".." || name.contains('\0') {
                return Err(FsError::Forbidden);
            }
            result.push(name);
            match fs::metadata_v2(result.as_str().as_bytes()).await {
                Ok(metadata) if components.peek().is_some() && !metadata.is_dir() => {
                    return Err(FsError::Forbidden);
                }
                Ok(_) => {}
                Err(fs::ERR_NOT_FOUND) if allow_missing && components.peek().is_none() => {}
                Err(code) => return Err(error(code)),
            }
        }
        Ok(result)
    }

    async fn mutable_path(&self, path: &DavPath, allow_missing: bool) -> FsResult<PathBuf> {
        let result = self.resolve(path, allow_missing).await?;
        if result == self.root {
            return Err(FsError::Forbidden);
        }
        Ok(result)
    }
}

impl DavFileSystem for ShareFs {
    fn open<'a>(
        &'a self,
        path: &'a DavPath,
        options: OpenOptions,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        Box::pin(async move {
            if options.size.is_some_and(|size| size > MAX_FILE_BYTES) {
                return Err(FsError::TooLarge);
            }
            let writable = options.write || options.append;
            if !options.read && !writable {
                return Err(FsError::Forbidden);
            }
            let target = self
                .mutable_path(path, options.create || options.create_new)
                .await?;
            let (mut metadata, mut bytes, existed) = match snapshot(&target, true).await {
                Ok((metadata, bytes)) => {
                    if !metadata.record.is_file() {
                        return Err(FsError::Forbidden);
                    }
                    if options.create_new {
                        return Err(FsError::Exists);
                    }
                    (metadata, bytes, true)
                }
                Err(FsError::NotFound) if options.create || options.create_new => (
                    ShareMetadata {
                        record: MetadataV2 {
                            kind: NodeKind::File,
                            len: 0,
                            file_id: 0,
                            root_id: 0,
                            revision: 0,
                        },
                        content_type: ContentTypeId::BLOB,
                    },
                    Vec::new(),
                    false,
                ),
                Err(error) => return Err(error),
            };
            // Partial/append and create=false writes must not overwrite an intervening
            // change or recreate a record deleted since open. Full ordinary PUT keeps
            // its existing last-writer semantics; the checked publisher retains the
            // destination's current type, key and identity under its namespace lease.
            let expected_sha256 = (existed && writable && (!options.truncate || !options.create))
                .then(|| Sha256::digest(&bytes).into());
            let dirty = writable && (!existed || options.truncate);
            if writable && options.truncate {
                bytes.clear();
            }
            metadata.record.len = bytes.len() as u64;
            let position = if options.append {
                bytes.len() as u64
            } else {
                0
            };
            Ok(Box::new(ShareFile {
                path: target,
                bytes,
                position,
                readable: options.read,
                writable,
                append: options.append,
                dirty,
                create_new: options.create_new,
                expected_sha256,
                metadata,
            }) as Box<dyn DavFile>)
        })
    }

    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        _: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        Box::pin(async move {
            let directory = self.resolve(path, false).await?;
            if !snapshot(&directory, false).await?.0.record.is_dir() {
                return Err(FsError::Forbidden);
            }
            let listing = fs::list_dir_typed(directory.as_str().as_bytes())
                .await
                .map_err(error)?;
            if listing.truncated {
                return Err(FsError::GeneralFailure);
            }
            let mut entries = Vec::new();
            for entry in listing.entries {
                if entry.name.is_empty()
                    || entry.name == "."
                    || entry.name == ".."
                    || entry.name.contains('/')
                    || entry.name.contains('\0')
                {
                    return Err(FsError::GeneralFailure);
                }
                let target = directory.join(&entry.name);
                let metadata = match snapshot(&target, false).await {
                    Ok((metadata, _)) => metadata,
                    Err(FsError::NotFound) => continue,
                    Err(error) => return Err(error),
                };
                entries.push(Ok(Box::new(ShareEntry {
                    name: entry.name.into_bytes(),
                    metadata,
                }) as Box<dyn DavDirEntry>));
            }
            Ok(Box::pin(stream::iter(entries)) as FsStream<Box<dyn DavDirEntry>>)
        })
    }

    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        Box::pin(async move {
            let target = self.resolve(path, false).await?;
            Ok(Box::new(snapshot(&target, false).await?.0) as Box<dyn DavMetaData>)
        })
    }

    fn create_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async move {
            let target = self.mutable_path(path, true).await?;
            fs::create_dir(target.as_str().as_bytes())
                .await
                .map_err(error)
        })
    }

    fn remove_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async move {
            let target = self.mutable_path(path, false).await?;
            if !snapshot(&target, false).await?.0.record.is_dir() {
                return Err(FsError::Forbidden);
            }
            // Kind and emptiness are checked again under the publication lease;
            // a child added after DAV's recursive traversal must not be deleted.
            fs::remove_empty_dir(target.as_str().as_bytes())
                .await
                .map_err(error)
        })
    }

    fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async move {
            let target = self.mutable_path(path, false).await?;
            if !snapshot(&target, false).await?.0.record.is_file() {
                return Err(FsError::Forbidden);
            }
            fs::remove_file(target.as_str().as_bytes())
                .await
                .map_err(error)
        })
    }

    fn rename<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async move {
            let source = self.mutable_path(from, false).await?;
            let destination = self.mutable_path(to, true).await?;
            fs::rename(source.as_str().as_bytes(), destination.as_str().as_bytes())
                .await
                .map_err(error)
        })
    }

    fn copy<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async move {
            let source = self.resolve(from, false).await?;
            let (metadata, bytes) = snapshot(&source, true).await?;
            if !metadata.record.is_file() {
                return Err(FsError::Forbidden);
            }
            let destination = self.mutable_path(to, true).await?;
            fs::write_file_typed_checked(
                destination.as_str().as_bytes(),
                &bytes,
                metadata.content_type,
                true,
                None,
            )
            .await
            .map_err(error)
        })
    }
}

struct ShareEntry {
    name: Vec<u8>,
    metadata: ShareMetadata,
}

impl DavDirEntry for ShareEntry {
    fn name(&self) -> Vec<u8> {
        self.name.clone()
    }
    fn metadata(&self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        Box::pin(async move { Ok(Box::new(self.metadata.clone()) as Box<dyn DavMetaData>) })
    }
}

#[derive(Debug)]
struct ShareFile {
    path: PathBuf,
    bytes: Vec<u8>,
    position: u64,
    readable: bool,
    writable: bool,
    append: bool,
    dirty: bool,
    create_new: bool,
    expected_sha256: Option<[u8; 32]>,
    metadata: ShareMetadata,
}

impl ShareFile {
    fn write(&mut self, bytes: &[u8]) -> FsResult<()> {
        if !self.writable {
            return Err(FsError::Forbidden);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        let start = if self.append {
            self.bytes.len() as u64
        } else {
            self.position
        };
        let end = start
            .checked_add(bytes.len() as u64)
            .ok_or(FsError::TooLarge)?;
        if end > MAX_FILE_BYTES {
            return Err(FsError::TooLarge);
        }
        let end_usize = usize::try_from(end).map_err(|_| FsError::TooLarge)?;
        if end_usize > self.bytes.len() {
            self.bytes
                .try_reserve(end_usize - self.bytes.len())
                .map_err(|_| FsError::InsufficientStorage)?;
            self.bytes.resize(end_usize, 0);
        }
        self.bytes[start as usize..end_usize].copy_from_slice(bytes);
        self.position = end;
        self.metadata.record.len = self.bytes.len() as u64;
        self.dirty = true;
        Ok(())
    }
}

impl DavFile for ShareFile {
    fn metadata(&mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        Box::pin(async move { Ok(Box::new(self.metadata.clone()) as Box<dyn DavMetaData>) })
    }
    fn write_buf(&mut self, mut buf: Box<dyn Buf + Send>) -> FsFuture<'_, ()> {
        Box::pin(async move {
            while buf.has_remaining() {
                let bytes = buf.chunk();
                self.write(bytes)?;
                let count = bytes.len();
                buf.advance(count);
            }
            Ok(())
        })
    }
    fn write_bytes(&mut self, bytes: Bytes) -> FsFuture<'_, ()> {
        Box::pin(async move { self.write(&bytes) })
    }
    fn read_bytes(&mut self, count: usize) -> FsFuture<'_, Bytes> {
        Box::pin(async move {
            if !self.readable {
                return Err(FsError::Forbidden);
            }
            let start = usize::try_from(self.position)
                .map_err(|_| FsError::TooLarge)?
                .min(self.bytes.len());
            let end = start
                .saturating_add(count.min(64 * 1024))
                .min(self.bytes.len());
            let bytes = Bytes::copy_from_slice(&self.bytes[start..end]);
            self.position = self.position.saturating_add((end - start) as u64);
            Ok(bytes)
        })
    }
    fn seek(&mut self, position: SeekFrom) -> FsFuture<'_, u64> {
        Box::pin(async move {
            let position = match position {
                SeekFrom::Start(position) => i128::from(position),
                SeekFrom::End(offset) => self.bytes.len() as i128 + i128::from(offset),
                SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            };
            if position < 0 {
                return Err(FsError::Forbidden);
            }
            if position > i128::from(MAX_FILE_BYTES) {
                return Err(FsError::TooLarge);
            }
            self.position = position as u64;
            Ok(self.position)
        })
    }
    fn flush(&mut self) -> FsFuture<'_, ()> {
        Box::pin(async move {
            if !self.dirty {
                return Ok(());
            }
            fs::write_file_typed_checked(
                self.path.as_str().as_bytes(),
                &self.bytes,
                self.metadata.content_type,
                self.create_new,
                self.expected_sha256,
            )
            .await
            .map_err(|code| {
                if self.create_new && code == fs::ERR_ALREADY_EXISTS {
                    FsError::PreconditionFailed
                } else {
                    error(code)
                }
            })?;
            self.dirty = false;
            self.create_new = false;
            if self.expected_sha256.is_some() {
                self.expected_sha256 = Some(Sha256::digest(&self.bytes).into());
            }
            Ok(())
        })
    }
}
