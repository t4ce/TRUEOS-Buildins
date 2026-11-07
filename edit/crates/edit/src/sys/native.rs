//! Native filesystem identity and launch preferences; no descriptor or loader ABI.

use stdext::platform::{fs::{self, File}, io, path::Path, prelude::*};
use stdext::{arena::Arena, collections::{BString, BVec}};

#[derive(Clone, PartialEq, Eq)]
pub struct FileId {
    root: u32,
    file: u64,
}

pub fn file_id(file: Option<&File>, path: &Path) -> io::Result<FileId> {
    let metadata = match file { Some(file) => file.metadata()?, None => fs::metadata(path)? };
    if metadata.file_id == 0 {
        return Err(io::Error::new(io::ErrorKind::Other, "filesystem record has no stable identity"));
    }
    Ok(FileId { root: metadata.root_id, file: metadata.file_id })
}

pub fn preferred_languages(arena: &Arena) -> BVec<'_, &'_ str> {
    let mut locales = BVec::empty();
    for key in ["LANGUAGE", "LC_ALL", "LANG"] {
        if let Ok(value) = trueos::env::var(key) && !value.is_empty() {
            let value = BString::from_str(arena, &value).leak();
            for byte in unsafe { value.as_bytes_mut() } {
                if *byte == b'_' { *byte = b'-'; }
            }
            locales.extend_sloppy(arena, value.split(':').filter(|part| !part.is_empty()));
            break;
        }
    }
    locales
}
