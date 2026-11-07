//! Download storage adapters with explicit durable-prefix accounting.
use crate::path::Path;
use alloc::{
    string::{String, ToString},
    vec::Vec,
};

#[cfg(target_os = "trueos")]
#[path = "native_storage.rs"]
mod native;
#[cfg(target_os = "trueos")]
pub(crate) use native::*;

#[cfg(not(target_os = "trueos"))]
pub(crate) type File = tokio::fs::File;

#[cfg(not(target_os = "trueos"))]
pub(crate) async fn file_len(path: &Path) -> Result<Option<u64>, String> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_file() => Ok(Some(metadata.len())),
        Ok(_) => Err("download destination is not a file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(not(target_os = "trueos"))]
pub(crate) async fn create_dir_all(path: &Path) -> Result<(), String> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "trueos"))]
pub(crate) async fn open(path: &Path, offset: u64) -> Result<File, String> {
    tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(offset > 0)
        .truncate(offset == 0)
        .open(path)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "trueos"))]
pub(crate) async fn read_limited(path: &Path, max: u64) -> Result<Option<Vec<u8>>, String> {
    let Some(len) = file_len(path).await? else {
        return Ok(None);
    };
    if len > max {
        return Ok(None);
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| error.to_string())?;
    Ok((bytes.len() as u64 <= max).then_some(bytes))
}

#[cfg(not(target_os = "trueos"))]
pub(crate) async fn remove_file_if_exists(path: &Path) -> Result<(), String> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}
