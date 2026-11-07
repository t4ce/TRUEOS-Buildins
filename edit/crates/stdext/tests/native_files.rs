//! Run the production native editor streams against a deterministic VFS boundary.
extern crate self as trueos;

mod platform {
    pub use std::{fmt, string, vec};
    pub mod io {
        pub use std::io::*;
        pub use std::io::{Error as StreamError, Result as StreamResult};
    }
    pub mod prelude { pub use std::prelude::rust_2024::*; }
    pub mod path { pub use trueos_path::{Path, PathBuf}; }
}

mod env {
    pub fn current_dir() -> std::io::Result<trueos_path::PathBuf> {
        Ok(trueos_path::PathBuf::from("/"))
    }
}

#[path = "../src/native_fs.rs"]
mod native_files;

pub mod async_fs {
    use std::{cell::RefCell, collections::{BTreeMap, BTreeSet}, future::Future, task::{Context, Poll, Waker}};
    pub use infer::ContentTypeId;
    pub const ERR_NOT_FOUND: i32 = -8;
    pub const ERR_ALREADY_EXISTS: i32 = -9;
    pub const ERR_BAD_PARAM: i32 = -4;
    pub const ERR_BAD_UTF8: i32 = -1;
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum NodeKind { File, Directory }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct MetadataV2 { pub kind: NodeKind, pub len: u64, pub file_id: u64, pub root_id: u32, pub revision: u64 }
    impl MetadataV2 {
        pub fn is_dir(self) -> bool { self.kind == NodeKind::Directory }
    }
    pub struct TypedMetadata { pub kind: NodeKind, pub content_type: ContentTypeId }
    #[derive(Clone)]
    struct Record { bytes: Vec<u8>, content_type: ContentTypeId, revision: u64 }
    #[derive(Default)]
    pub struct State {
        records: BTreeMap<String, Record>,
        directories: BTreeSet<String>,
        pub fail_write: bool,
        pub replace_on_read: usize,
        pub write_calls: usize,
    }
    thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
    pub fn reset() { STATE.with(|state| *state.borrow_mut() = State::default()); }
    pub fn seed(path: &str, bytes: &[u8], content_type: ContentTypeId) {
        STATE.with(|state| { state.borrow_mut().records.insert(path.into(), Record { bytes: bytes.into(), content_type, revision: 1 }); });
    }
    pub fn change(f: impl FnOnce(&mut State)) { STATE.with(|state| f(&mut state.borrow_mut())); }
    pub fn bytes(path: &str) -> Vec<u8> { STATE.with(|state| state.borrow().records[path].bytes.clone()) }
    pub fn content_type(path: &str) -> ContentTypeId { STATE.with(|state| state.borrow().records[path].content_type) }
    fn path(path: &[u8]) -> Result<&str, i32> { std::str::from_utf8(path).map_err(|_| ERR_BAD_UTF8) }
    pub fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut context) { Poll::Ready(value) => value, Poll::Pending => panic!("mock unexpectedly parked") }
    }
    pub async fn metadata_v2(name: &[u8]) -> Result<MetadataV2, i32> {
        let name = path(name)?;
        STATE.with(|state| {
            let state = state.borrow();
            if state.directories.contains(name) { return Ok(MetadataV2 { kind: NodeKind::Directory, len: 0, file_id: 8, root_id: 3, revision: 1 }); }
            let record = state.records.get(name).ok_or(ERR_NOT_FOUND)?;
            Ok(MetadataV2 { kind: NodeKind::File, len: record.bytes.len() as u64, file_id: 7, root_id: 3, revision: record.revision })
        })
    }
    pub async fn typed_metadata(name: &[u8]) -> Result<TypedMetadata, i32> {
        let name = path(name)?;
        STATE.with(|state| {
            let state = state.borrow();
            let record = state.records.get(name).ok_or(ERR_NOT_FOUND)?;
            Ok(TypedMetadata { kind: NodeKind::File, content_type: record.content_type })
        })
    }
    pub async fn read_file(name: &[u8]) -> Result<Vec<u8>, i32> {
        let name = path(name)?;
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let bytes = state.records.get(name).ok_or(ERR_NOT_FOUND)?.bytes.clone();
            if state.replace_on_read > 0 {
                state.replace_on_read -= 1;
                let record = state.records.get_mut(name).unwrap();
                record.bytes = b"replacement".to_vec();
                record.revision += 1;
            }
            Ok(bytes)
        })
    }
    pub async fn create_dir_all(name: &[u8]) -> Result<(), i32> {
        let name = path(name)?;
        STATE.with(|state| { state.borrow_mut().directories.insert(name.into()); });
        Ok(())
    }
    pub async fn write_file_typed_checked(name: &[u8], bytes: &[u8], content_type: ContentTypeId, create_new: bool, _: Option<[u8; 32]>) -> Result<(), i32> {
        let name = path(name)?;
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            if state.fail_write { return Err(-2); }
            if create_new && state.records.contains_key(name) { return Err(ERR_ALREADY_EXISTS); }
            let revision = state.records.get(name).map_or(1, |record| record.revision + 1);
            state.records.insert(name.into(), Record { bytes: bytes.into(), content_type, revision });
            state.write_calls += 1;
            Ok(())
        })
    }
}

use std::io::{Read, Write};
use native_files::File;

#[test]
fn typed_save_retains_identity_and_requires_successful_commit() {
    async_fs::reset();
    async_fs::seed("/page.html", b"<html>old</html>", infer::ContentTypeId::HTML);
    let mut file = File::create("/page.html").unwrap();
    file.write_all(b"<html>new</html>").unwrap();
    assert_eq!(async_fs::bytes("/page.html"), b"<html>old</html>");
    async_fs::change(|state| state.fail_write = true);
    assert!(file.commit().is_err());
    assert_eq!(async_fs::bytes("/page.html"), b"<html>old</html>");
    async_fs::change(|state| state.fail_write = false);
    file.commit().unwrap();
    assert_eq!(async_fs::bytes("/page.html"), b"<html>new</html>");
    assert_eq!(async_fs::content_type("/page.html"), infer::ContentTypeId::HTML);
    async_fs::change(|state| assert_eq!(state.write_calls, 1));
}

#[test]
fn dropping_uncommitted_output_keeps_previous_bytes() {
    async_fs::reset();
    async_fs::seed("/file.txt", b"old", infer::ContentTypeId::UTF8_TEXT);
    let mut file = File::create("/file.txt").unwrap();
    file.write_all(b"discarded").unwrap();
    drop(file);
    assert_eq!(async_fs::bytes("/file.txt"), b"old");
}

#[test]
fn opening_retries_replacement_and_reports_matching_metadata() {
    async_fs::reset();
    async_fs::seed("/file.txt", &[b'x'; 8192], infer::ContentTypeId::UTF8_TEXT);
    async_fs::change(|state| state.replace_on_read = 1);
    let mut file = File::open("/file.txt").unwrap();
    let metadata = file.metadata().unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"replacement");
    assert_eq!(metadata.len, bytes.len() as u64);
    assert_eq!((metadata.root_id, metadata.file_id, metadata.revision), (3, 7, 2));
}

#[test]
fn opening_a_continuously_changing_file_fails_without_partial_stream() {
    async_fs::reset();
    async_fs::seed("/file.txt", b"old", infer::ContentTypeId::UTF8_TEXT);
    async_fs::change(|state| state.replace_on_read = 3);
    assert!(matches!(File::open("/file.txt"), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
}

#[test]
fn a_new_file_is_create_only_and_gets_a_text_identity() {
    async_fs::reset();
    let mut file = File::create("/new.txt").unwrap();
    file.write_all(b"hello").unwrap();
    async_fs::seed("/new.txt", b"racing file", infer::ContentTypeId::UTF8_TEXT);
    assert_eq!(file.commit().unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(async_fs::bytes("/new.txt"), b"racing file");
    let mut file = File::create("/other.txt").unwrap();
    file.write_all(b"hello").unwrap();
    file.commit().unwrap();
    assert_eq!(async_fs::content_type("/other.txt"), infer::ContentTypeId::UTF8_TEXT);
}

#[test]
fn compiler_input_directory_cannot_be_reused() {
    async_fs::reset();
    native_files::create_dir("/build/123").unwrap();
    assert_eq!(native_files::create_dir("/build/123").unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
}
