#![cfg_attr(target_os = "trueos", no_std)]
extern crate alloc;

#[cfg_attr(target_os = "trueos", path = "native_filesystem.rs")]
mod filesystem;
pub mod http;

use alloc::boxed::Box;
#[cfg(not(target_os = "trueos"))]
use std::io;
#[cfg(not(target_os = "trueos"))]
use std::path::Path;
#[cfg(target_os = "trueos")]
use trueos_path::Path;
#[cfg(target_os = "trueos")]
use trueos_runtime::io;

use dav_server::{DavHandler, memls::MemLs};

/// Build an unauthenticated, read/write share rooted at one directory.
pub async fn handler(root: impl AsRef<Path>) -> io::Result<DavHandler> {
    Ok(DavHandler::builder()
        .filesystem(Box::new(filesystem::ShareFs::new(root.as_ref()).await?))
        .locksystem(MemLs::new())
        .protect_root(true)
        .hide_symlinks(true)
        .autoindex(true)
        .build_handler())
}
