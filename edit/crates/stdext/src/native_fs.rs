//! Synchronous editor streams backed by native, explicitly committed VFS jobs.

use crate::platform::{io::{self, Read, Write}, path::{Path, PathBuf}, prelude::*};

pub use trueos::async_fs::MetadataV2 as Metadata;

fn error(code: i32) -> io::Error {
    let kind = match code {
        trueos::async_fs::ERR_NOT_FOUND => io::ErrorKind::NotFound,
        trueos::async_fs::ERR_ALREADY_EXISTS => io::ErrorKind::AlreadyExists,
        trueos::async_fs::ERR_BAD_PARAM | trueos::async_fs::ERR_BAD_UTF8 => io::ErrorKind::InvalidInput,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, format!("native filesystem request failed ({code})"))
}

pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    trueos::async_fs::block_on(trueos::async_fs::metadata_v2(path.as_ref().as_str().as_bytes())).map_err(error)
}
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    trueos::async_fs::block_on(trueos::async_fs::read_file(path.as_ref().as_str().as_bytes())).map_err(error)
}
pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    trueos::async_fs::block_on(trueos::async_fs::create_dir_all(path.as_ref().as_str().as_bytes())).map_err(error)
}
pub fn write(path: impl AsRef<Path>, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.commit()
}

/// Reserve a fresh build input directory. The create-only marker serializes
/// callers racing directory creation; existing input snapshots are never reused.
pub fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    match metadata(path) {
        Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "build directory already exists")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {},
        Err(error) => return Err(error),
    }
    create_dir_all(path)?;
    let marker = path.join(".edit-build-reservation");
    trueos::async_fs::block_on(trueos::async_fs::write_file_typed_checked(
        marker.as_str().as_bytes(), b"immutable Edit build inputs\n",
        trueos::async_fs::ContentTypeId::UTF8_TEXT, true, None,
    )).map_err(error)
}
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    metadata(path)?;
    let absolute = if path.is_absolute() { path.to_path_buf() } else { super::env::current_dir()?.join(path) };
    Ok(absolute.normalize())
}

pub struct File {
    path: PathBuf,
    bytes: Vec<u8>,
    position: usize,
    writable: bool,
    dirty: bool,
    snapshot: Option<Metadata>,
    content_type: Option<trueos::async_fs::ContentTypeId>,
}
impl File {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        // Pair stream bytes with their committed identity and revision. A live
        // pathname can change between requests; never report the replacement's
        // length for bytes captured from its predecessor.
        for _ in 0..3 {
            let before = metadata(path)?;
            let bytes = read(path)?;
            let mut after = metadata(path)?;
            if before == after && after.len == bytes.len() as u64 {
                after.len = bytes.len() as u64;
                return Ok(Self { path: path.to_path_buf(), bytes, position: 0,
                    writable: false, dirty: false, snapshot: Some(after), content_type: None });
            }
        }
        Err(io::Error::new(io::ErrorKind::WouldBlock, "file changed while opening; retry"))
    }
    pub fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let content_type = match trueos::async_fs::block_on(trueos::async_fs::typed_metadata(path.as_str().as_bytes())) {
            Ok(metadata) if matches!(metadata.kind, trueos::async_fs::NodeKind::File) => Some(metadata.content_type),
            Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot replace a directory with text")),
            Err(trueos::async_fs::ERR_NOT_FOUND) => None,
            Err(code) => return Err(error(code)),
        };
        Ok(Self { path: path.to_path_buf(), bytes: Vec::new(), position: 0,
            writable: true, dirty: true, snapshot: None, content_type })
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        match self.snapshot { Some(snapshot) => Ok(snapshot), None => metadata(&self.path) }
    }
    /// Never commit from Drop: the caller must observe durable-write failures.
    pub fn commit(&mut self) -> io::Result<()> {
        if self.dirty {
            let content_type = self.content_type.unwrap_or_else(|| {
                infer::detect_content_type(&self.bytes).unwrap_or(trueos::async_fs::ContentTypeId::UTF8_TEXT)
            });
            // Checked replacement preserves the existing record's declared type
            // and access key under the kernel's publication lease.
            trueos::async_fs::block_on(trueos::async_fs::write_file_typed_checked(
                self.path.as_str().as_bytes(), &self.bytes, content_type,
                self.content_type.is_none(), None,
            )).map_err(error)?;
            self.content_type = Some(content_type);
            self.dirty = false;
        }
        Ok(())
    }
}
impl Read for File {
    fn read(&mut self, out: &mut [u8]) -> io::StreamResult<usize> {
        let count = out.len().min(self.bytes.len().saturating_sub(self.position));
        out[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}
impl Write for File {
    fn write(&mut self, bytes: &[u8]) -> io::StreamResult<usize> {
        if !self.writable { return Err(io::StreamError::new(io::ErrorKind::PermissionDenied, "file opened for reading")); }
        self.bytes.try_reserve(bytes.len()).map_err(|_| io::StreamError::new(io::ErrorKind::Other, "editor write buffer allocation failed"))?;
        self.bytes.extend_from_slice(bytes);
        self.dirty = true;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::StreamResult<()> {
        self.commit().map_err(|_| io::StreamError::new(io::ErrorKind::Other, "native write commit failed"))
    }
}
