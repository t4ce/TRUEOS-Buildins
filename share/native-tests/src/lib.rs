//! Execute the production native storage adapter with deterministic native services.
extern crate alloc;
extern crate self as trueos;
extern crate self as trueos_runtime;
pub use std::io;

#[cfg(test)]
#[path = "../../src/native_filesystem.rs"]
mod filesystem;

#[allow(dead_code)]
pub mod async_fs {
    use sha2::{Digest, Sha256};
    use std::{cell::RefCell, collections::BTreeMap};
    pub const ERR_NOT_FOUND: i32 = -8;
    pub const ERR_ALREADY_EXISTS: i32 = -9;
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum NodeKind {
        File,
        Directory,
    }
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct ContentTypeId(pub u16);
    impl ContentTypeId {
        pub const BLOB: Self = Self(1);
        pub const UTF8_TEXT: Self = Self(2);
        pub const PNG: Self = Self(3);
    }
    pub struct ContentTypeInfo {
        pub mime_type: &'static str,
    }
    pub fn content_type_info(id: ContentTypeId) -> Option<&'static ContentTypeInfo> {
        match id {
            ContentTypeId::UTF8_TEXT => Some(&ContentTypeInfo {
                mime_type: "text/plain",
            }),
            ContentTypeId::PNG => Some(&ContentTypeInfo {
                mime_type: "image/png",
            }),
            _ => None,
        }
    }
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct MetadataV2 {
        pub kind: NodeKind,
        pub len: u64,
        pub file_id: u64,
        pub root_id: u32,
        pub revision: u64,
    }
    impl MetadataV2 {
        pub fn is_file(self) -> bool {
            self.kind == NodeKind::File
        }
        pub fn is_dir(self) -> bool {
            self.kind == NodeKind::Directory
        }
    }
    pub struct TypedMetadata {
        pub kind: NodeKind,
        pub len: u64,
        pub content_type: ContentTypeId,
    }
    pub struct TypedDirEntry {
        pub name: String,
    }
    pub struct TypedDirListing {
        pub entries: Vec<TypedDirEntry>,
        pub truncated: bool,
    }
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Record {
        pub bytes: Vec<u8>,
        pub kind: NodeKind,
        pub content_type: ContentTypeId,
        pub id: u64,
        pub revision: u64,
        pub key: u64,
    }
    #[derive(Default)]
    struct Store {
        records: BTreeMap<String, Record>,
        next_id: u64,
        pub commits: usize,
        pub fail_commit: Option<i32>,
        replace_during_read: Option<(String, Record)>,
        truncate_listing: bool,
    }
    thread_local! { static STORE: RefCell<Store> = RefCell::default(); }
    fn path(bytes: &[u8]) -> &str {
        std::str::from_utf8(bytes).unwrap()
    }
    fn directory() -> Record {
        Record {
            bytes: Vec::new(),
            kind: NodeKind::Directory,
            content_type: ContentTypeId::BLOB,
            id: 1,
            revision: 1,
            key: 0,
        }
    }
    pub fn reset() {
        STORE.with(|s| {
            let mut s = s.borrow_mut();
            *s = Store::default();
            s.next_id = 10;
            s.records.insert("root".into(), directory());
        });
    }
    pub fn put(name: &str, bytes: &[u8], content_type: ContentTypeId, key: u64) {
        STORE.with(|s| {
            let mut s = s.borrow_mut();
            s.next_id += 1;
            let id = s.next_id;
            s.records.insert(
                name.into(),
                Record {
                    bytes: bytes.to_vec(),
                    kind: NodeKind::File,
                    content_type,
                    id,
                    revision: 1,
                    key,
                },
            );
        });
    }
    pub fn get(name: &str) -> Option<Record> {
        STORE.with(|s| s.borrow().records.get(name).cloned())
    }
    pub fn records() -> BTreeMap<String, Record> {
        STORE.with(|s| s.borrow().records.clone())
    }
    pub fn fail_commit(code: i32) {
        STORE.with(|s| s.borrow_mut().fail_commit = Some(code));
    }
    pub fn commits() -> usize {
        STORE.with(|s| s.borrow().commits)
    }
    pub fn replace_during_read(name: &str, bytes: &[u8], content_type: ContentTypeId) {
        let old = get(name).unwrap();
        STORE.with(|s| {
            s.borrow_mut().replace_during_read = Some((
                name.into(),
                Record {
                    bytes: bytes.to_vec(),
                    content_type,
                    id: old.id + 100,
                    revision: 1,
                    ..old
                },
            ))
        });
    }
    pub fn truncate_listing() {
        STORE.with(|s| s.borrow_mut().truncate_listing = true);
    }
    pub async fn create_dir_all(bytes: &[u8]) -> Result<(), i32> {
        STORE.with(|s| {
            let mut s = s.borrow_mut();
            if let Some(record) = s.records.get(path(bytes)) {
                return if record.kind == NodeKind::Directory {
                    Ok(())
                } else {
                    Err(-6)
                };
            }
            s.records.insert(path(bytes).into(), directory());
            Ok(())
        })
    }
    pub async fn metadata_v2(bytes: &[u8]) -> Result<MetadataV2, i32> {
        let record = get(path(bytes)).ok_or(-8)?;
        Ok(MetadataV2 {
            kind: record.kind,
            len: record.bytes.len() as u64,
            file_id: record.id,
            root_id: 1,
            revision: record.revision,
        })
    }
    pub async fn typed_metadata(bytes: &[u8]) -> Result<TypedMetadata, i32> {
        let record = get(path(bytes)).ok_or(-8)?;
        Ok(TypedMetadata {
            kind: record.kind,
            len: record.bytes.len() as u64,
            content_type: record.content_type,
        })
    }
    pub async fn read_file(bytes: &[u8]) -> Result<Vec<u8>, i32> {
        STORE.with(|s| {
            let mut s = s.borrow_mut();
            let record = s.records.get(path(bytes)).ok_or(-8)?.clone();
            if let Some((name, replacement)) = s.replace_during_read.take() {
                assert_eq!(name, path(bytes));
                s.records.insert(name, replacement);
            }
            Ok(record.bytes)
        })
    }
    pub async fn write_file_typed_checked(
        bytes: &[u8],
        data: &[u8],
        declared: ContentTypeId,
        create_new: bool,
        expected: Option<[u8; 32]>,
    ) -> Result<(), i32> {
        STORE.with(|s| {
            let mut s = s.borrow_mut();
            if let Some(code) = s.fail_commit.take() {
                return Err(code);
            }
            let name = path(bytes);
            if create_new && s.records.contains_key(name) {
                return Err(-9);
            }
            if let Some(expected) = expected {
                let record = s.records.get(name).ok_or(-17)?;
                if <[u8; 32]>::from(Sha256::digest(&record.bytes)) != expected {
                    return Err(-17);
                }
            }
            let parent = name.rsplit_once('/').unwrap().0;
            if s.records.get(parent).ok_or(-8)?.kind != NodeKind::Directory {
                return Err(-6);
            }
            s.next_id += 1;
            let next_id = s.next_id;
            let record = s.records.entry(name.into()).or_insert(Record {
                bytes: Vec::new(),
                kind: NodeKind::File,
                content_type: declared,
                id: next_id,
                revision: 0,
                key: 0,
            });
            if record.kind != NodeKind::File {
                return Err(-6);
            }
            record.bytes = data.to_vec();
            record.revision += 1;
            s.commits += 1;
            Ok(())
        })
    }
    pub async fn list_dir_typed(bytes: &[u8]) -> Result<TypedDirListing, i32> {
        STORE.with(|s| {
            let s = s.borrow();
            let name = path(bytes);
            let prefix = format!("{name}/");
            if s.records.get(name).ok_or(-8)?.kind != NodeKind::Directory {
                return Err(-6);
            }
            Ok(TypedDirListing {
                truncated: s.truncate_listing,
                entries: s
                    .records
                    .keys()
                    .filter_map(|name| name.strip_prefix(&prefix))
                    .filter(|name| !name.contains('/'))
                    .map(|name| TypedDirEntry { name: name.into() })
                    .collect(),
            })
        })
    }
    pub async fn create_dir(bytes: &[u8]) -> Result<(), i32> {
        STORE.with(|s| {
            let mut s = s.borrow_mut();
            let name = path(bytes);
            if s.records.contains_key(name) {
                return Err(-9);
            }
            let parent = name.rsplit_once('/').ok_or(-6)?.0;
            if s.records.get(parent).ok_or(-8)?.kind != NodeKind::Directory {
                return Err(-6);
            }
            s.records.insert(name.into(), directory());
            Ok(())
        })
    }
    pub async fn remove_file(bytes: &[u8]) -> Result<(), i32> {
        remove(bytes, false)
    }
    pub async fn remove_empty_dir(bytes: &[u8]) -> Result<(), i32> {
        remove(bytes, true)
    }
    fn remove(bytes: &[u8], directory: bool) -> Result<(), i32> {
        STORE.with(|s| {
            let mut s = s.borrow_mut();
            let name = path(bytes);
            if (s.records.get(name).ok_or(-8)?.kind == NodeKind::Directory) != directory {
                return Err(-6);
            }
            if directory && s.records.keys().any(|p| p.starts_with(&format!("{name}/"))) {
                return Err(-9);
            }
            s.records.remove(name);
            Ok(())
        })
    }
    pub async fn rename(from: &[u8], to: &[u8]) -> Result<(), i32> {
        STORE.with(|s| {
            let mut s = s.borrow_mut();
            if s.records.contains_key(path(to)) {
                return Err(-9);
            }
            let record = s.records.remove(path(from)).ok_or(-8)?;
            s.records.insert(path(to).into(), record);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        async_fs as fs,
        filesystem::{MAX_FILE_BYTES, ShareFs},
    };
    use bytes::Bytes;
    use dav_server::{
        DavHandler,
        davpath::DavPath,
        fs::{DavFileSystem, FsError, OpenOptions, ReadDirMeta, SeekFrom},
    };
    use http::{Method, Request, StatusCode};
    use http_body_util::{BodyExt, Full};
    use trueos_path::Path;

    fn path(text: &str) -> DavPath {
        DavPath::new(text).unwrap()
    }
    async fn backend() -> ShareFs {
        fs::reset();
        ShareFs::new(Path::new("root")).await.unwrap()
    }
    fn read() -> OpenOptions {
        OpenOptions {
            read: true,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
            size: None,
            checksum: None,
        }
    }
    fn put() -> OpenOptions {
        OpenOptions {
            read: false,
            write: true,
            append: false,
            truncate: true,
            create: true,
            create_new: false,
            size: None,
            checksum: None,
        }
    }

    #[tokio::test]
    async fn snapshot_retries_replaced_bytes_and_type_and_stays_stable_after_open() {
        let backend = backend().await;
        fs::put("root/a", b"old", fs::ContentTypeId::UTF8_TEXT, 44);
        fs::replace_during_read("root/a", b"\0PNG bytes\xff", fs::ContentTypeId::PNG);
        let mut file = backend.open(&path("/a"), read()).await.unwrap();
        fs::put("root/a", b"changed later", fs::ContentTypeId::BLOB, 55);
        let metadata = file.metadata().await.unwrap();
        assert_eq!(metadata.content_type(), Some("image/png"));
        assert_eq!(metadata.len(), 11);
        assert_eq!(&file.read_bytes(99).await.unwrap()[..], b"\0PNG bytes\xff");
        assert!(metadata.modified().is_err());
        assert_eq!(metadata.etag(), None);
    }

    #[tokio::test]
    async fn replacing_typed_record_waits_for_commit_and_retains_identity_and_key() {
        let backend = backend().await;
        fs::put("root/note", b"before", fs::ContentTypeId::UTF8_TEXT, 88);
        let old = fs::get("root/note").unwrap();
        let mut file = backend.open(&path("/note"), put()).await.unwrap();
        file.write_bytes(Bytes::from_static(b"\xef\xbb\xbfafter"))
            .await
            .unwrap();
        assert_eq!(fs::get("root/note").unwrap().bytes, b"before");
        fs::fail_commit(-2);
        assert_eq!(file.flush().await, Err(FsError::GeneralFailure));
        assert_eq!(fs::get("root/note").unwrap().bytes, b"before");
        file.flush().await.unwrap();
        let new = fs::get("root/note").unwrap();
        assert_eq!(new.bytes, b"\xef\xbb\xbfafter");
        assert_eq!(
            (new.id, new.key, new.content_type),
            (old.id, old.key, old.content_type)
        );
        assert_eq!(fs::commits(), 1);
        file.flush().await.unwrap();
        assert_eq!(fs::commits(), 1);
    }

    #[tokio::test]
    async fn abandoned_and_empty_uploads_have_explicit_outcomes() {
        let backend = backend().await;
        let mut file = backend.open(&path("/new"), put()).await.unwrap();
        file.write_bytes(Bytes::from_static(b"incomplete"))
            .await
            .unwrap();
        drop(file);
        assert!(fs::get("root/new").is_none());
        assert_eq!(fs::commits(), 0);
        let mut file = backend.open(&path("/new"), put()).await.unwrap();
        file.flush().await.unwrap();
        let new = fs::get("root/new").unwrap();
        assert!(new.bytes.is_empty());
        assert_eq!(new.content_type, fs::ContentTypeId::BLOB);
    }

    #[tokio::test]
    async fn create_only_and_partial_writes_detect_racing_publication() {
        let backend = backend().await;
        let mut options = put();
        options.create_new = true;
        let mut file = backend.open(&path("/new"), options).await.unwrap();
        file.write_bytes(Bytes::from_static(b"mine")).await.unwrap();
        fs::put("root/new", b"winner", fs::ContentTypeId::PNG, 10);
        assert_eq!(file.flush().await, Err(FsError::PreconditionFailed));
        assert_eq!(fs::get("root/new").unwrap().bytes, b"winner");

        fs::put("root/partial", b"abcdef", fs::ContentTypeId::UTF8_TEXT, 22);
        let mut options = put();
        options.truncate = false;
        let mut file = backend.open(&path("/partial"), options).await.unwrap();
        file.seek(SeekFrom::Start(2)).await.unwrap();
        file.write_bytes(Bytes::from_static(b"XY")).await.unwrap();
        fs::put("root/partial", b"racing", fs::ContentTypeId::UTF8_TEXT, 22);
        assert_eq!(file.flush().await, Err(FsError::PreconditionFailed));
        assert_eq!(fs::get("root/partial").unwrap().bytes, b"racing");
    }

    #[tokio::test]
    async fn partial_append_holes_and_create_false_preserve_bytes_or_fail_without_recreation() {
        let backend = backend().await;
        fs::put("root/partial", b"abcdef", fs::ContentTypeId::UTF8_TEXT, 9);
        let mut options = put();
        options.truncate = false;
        options.create = false;
        let mut file = backend.open(&path("/partial"), options).await.unwrap();
        file.seek(SeekFrom::Start(2)).await.unwrap();
        file.write_bytes(Bytes::from_static(b"XY")).await.unwrap();
        file.flush().await.unwrap();
        assert_eq!(fs::get("root/partial").unwrap().bytes, b"abXYef");
        let mut options = put();
        options.truncate = false;
        options.append = true;
        let mut file = backend.open(&path("/partial"), options).await.unwrap();
        file.seek(SeekFrom::Start(0)).await.unwrap();
        file.write_bytes(Bytes::from_static(b"!")).await.unwrap();
        file.flush().await.unwrap();
        assert_eq!(fs::get("root/partial").unwrap().bytes, b"abXYef!");
        let mut options = put();
        options.create = false;
        let mut file = backend.open(&path("/partial"), options).await.unwrap();
        file.write_bytes(Bytes::from_static(b"do not recreate"))
            .await
            .unwrap();
        fs::remove_file(b"root/partial").await.unwrap();
        assert_eq!(file.flush().await, Err(FsError::PreconditionFailed));
        assert!(fs::get("root/partial").is_none());
        let mut file = backend.open(&path("/hole"), put()).await.unwrap();
        file.seek(SeekFrom::Start(3)).await.unwrap();
        file.write_bytes(Bytes::from_static(b"!")).await.unwrap();
        file.flush().await.unwrap();
        assert_eq!(fs::get("root/hole").unwrap().bytes, b"\0\0\0!");
    }

    #[tokio::test]
    async fn copy_has_source_type_new_identity_and_move_keeps_record_identity() {
        let backend = backend().await;
        fs::put("root/a", b"\0\xffimage", fs::ContentTypeId::PNG, 77);
        let original = fs::get("root/a").unwrap();
        backend.copy(&path("/a"), &path("/copy")).await.unwrap();
        let copy = fs::get("root/copy").unwrap();
        assert_eq!(copy.bytes, original.bytes);
        assert_eq!(copy.content_type, original.content_type);
        assert_ne!(copy.id, original.id);
        assert_eq!(copy.key, 0);
        backend.rename(&path("/a"), &path("/moved")).await.unwrap();
        let moved = fs::get("root/moved").unwrap();
        assert_eq!(
            (moved.id, moved.key, moved.content_type),
            (original.id, original.key, original.content_type)
        );
        assert!(fs::get("root/a").is_none());
        assert_eq!(
            backend.copy(&path("/copy"), &path("/moved")).await,
            Err(FsError::Exists)
        );
        assert_eq!(fs::get("root/moved").unwrap().bytes, original.bytes);
    }

    #[tokio::test]
    async fn directory_and_file_removals_keep_kind_and_root_boundaries() {
        let backend = backend().await;
        assert_eq!(
            backend.remove_dir(&path("/")).await,
            Err(FsError::Forbidden)
        );
        backend.create_dir(&path("/dir")).await.unwrap();
        assert_eq!(
            backend.create_dir(&path("/dir")).await,
            Err(FsError::Exists)
        );
        assert_eq!(
            backend.create_dir(&path("/missing/leaf")).await,
            Err(FsError::NotFound)
        );
        fs::put("root/dir/child", b"keep", fs::ContentTypeId::BLOB, 0);
        assert_eq!(
            backend.remove_dir(&path("/dir")).await,
            Err(FsError::Exists)
        );
        assert_eq!(
            backend.remove_file(&path("/dir")).await,
            Err(FsError::Forbidden)
        );
        assert_eq!(
            backend.remove_dir(&path("/dir/child")).await,
            Err(FsError::Forbidden)
        );
        backend.remove_file(&path("/dir/child")).await.unwrap();
        backend.remove_dir(&path("/dir")).await.unwrap();
        assert!(fs::get("root/dir").is_none());
    }

    #[tokio::test]
    async fn size_seek_and_truncated_listing_fail_explicitly() {
        let backend = backend().await;
        let mut options = put();
        options.size = Some(MAX_FILE_BYTES + 1);
        assert!(matches!(
            backend.open(&path("/big"), options).await,
            Err(FsError::TooLarge)
        ));
        let mut file = backend.open(&path("/small"), put()).await.unwrap();
        assert_eq!(
            file.seek(SeekFrom::Current(-1)).await,
            Err(FsError::Forbidden)
        );
        assert_eq!(
            file.seek(SeekFrom::Start(MAX_FILE_BYTES + 1)).await,
            Err(FsError::TooLarge)
        );
        file.seek(SeekFrom::Start(MAX_FILE_BYTES)).await.unwrap();
        assert_eq!(
            file.write_bytes(Bytes::from_static(b"x")).await,
            Err(FsError::TooLarge)
        );
        fs::truncate_listing();
        assert!(matches!(
            backend.read_dir(&path("/"), ReadDirMeta::Data).await,
            Err(FsError::GeneralFailure)
        ));
        assert_eq!(fs::commits(), 0);
    }

    #[tokio::test]
    async fn short_http_body_never_commits_and_get_uses_stored_type() {
        let backend = backend().await;
        fs::put("root/note.png", b"old", fs::ContentTypeId::UTF8_TEXT, 99);
        let handler = DavHandler::builder()
            .protect_root(true)
            .filesystem(Box::new(backend))
            .build_handler();
        let request = Request::builder()
            .method(Method::PUT)
            .uri("/note.png")
            .header("content-length", "10")
            .body(Full::new(Bytes::from_static(b"short")))
            .unwrap();
        let response = handler.handle(request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(fs::get("root/note.png").unwrap().bytes, b"old");
        assert_eq!(fs::commits(), 0);
        let response = handler
            .handle(
                Request::builder()
                    .method(Method::GET)
                    .uri("/note.png")
                    .body(Full::new(Bytes::new()))
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/plain");
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            b"old"[..]
        );
    }

    #[tokio::test]
    async fn http_commit_error_preserves_old_record_and_success_keeps_stored_metadata() {
        let backend = backend().await;
        fs::put("root/note", b"before", fs::ContentTypeId::UTF8_TEXT, 99);
        let before = fs::get("root/note").unwrap();
        let handler = DavHandler::builder()
            .protect_root(true)
            .filesystem(Box::new(backend))
            .build_handler();
        fs::fail_commit(-2);
        let request = || {
            Request::builder()
                .method(Method::PUT)
                .uri("/note")
                .header("content-length", "5")
                .header("content-type", "image/png")
                .body(Full::new(Bytes::from_static(b"after")))
                .unwrap()
        };
        let response = handler.handle(request()).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(fs::get("root/note").unwrap().bytes, b"before");
        assert_eq!(fs::commits(), 0);
        let response = handler.handle(request()).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let after = fs::get("root/note").unwrap();
        assert_eq!(after.bytes, b"after");
        assert_eq!(
            (after.id, after.key, after.content_type),
            (before.id, before.key, before.content_type)
        );
        assert_eq!(fs::commits(), 1);
    }

    #[tokio::test]
    async fn http_foreign_upload_declares_blob_and_propfind_uses_stored_mime() {
        let backend = backend().await;
        let handler = DavHandler::builder()
            .protect_root(true)
            .filesystem(Box::new(backend))
            .build_handler();
        let response = handler
            .handle(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/photo.png")
                    .header("content-length", "3")
                    .header("content-type", "image/png")
                    .body(Full::new(Bytes::from_static(b"raw")))
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            fs::get("root/photo.png").unwrap().content_type,
            fs::ContentTypeId::BLOB
        );
        let response = handler
            .handle(
                Request::builder()
                    .method("PROPFIND")
                    .uri("/photo.png")
                    .header("depth", "0")
                    .body(Full::new(Bytes::new()))
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::MULTI_STATUS);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let xml = std::str::from_utf8(&body).unwrap();
        assert!(xml.contains("application/octet-stream"), "{xml}");
        assert!(!xml.contains("image/png"), "{xml}");
    }

    #[tokio::test]
    async fn root_mutations_reject_before_traversal_with_populated_typed_records() {
        let backend = backend().await;
        backend.create_dir(&path("/nested")).await.unwrap();
        fs::put(
            "root/source.txt",
            b"source",
            fs::ContentTypeId::UTF8_TEXT,
            11,
        );
        fs::put(
            "root/victim.png",
            b"\x00\xffbinary",
            fs::ContentTypeId::PNG,
            22,
        );
        fs::put("root/nested/child", b"nested", fs::ContentTypeId::BLOB, 33);
        let before = fs::records();
        let handler = DavHandler::builder()
            .protect_root(true)
            .filesystem(Box::new(backend))
            .build_handler();
        for (method, source, destination) in [
            (Method::DELETE, "/", None),
            (
                Method::from_bytes(b"MOVE").unwrap(),
                "/",
                Some("/victim.png"),
            ),
            (
                Method::from_bytes(b"COPY").unwrap(),
                "/source.txt",
                Some("/"),
            ),
            (
                Method::from_bytes(b"MOVE").unwrap(),
                "/source.txt",
                Some("/"),
            ),
        ] {
            let mut request = Request::builder().method(method.clone()).uri(source);
            if let Some(destination) = destination {
                request = request.header("Destination", destination);
            }
            let response = handler
                .handle(request.body(Full::new(Bytes::new())).unwrap())
                .await;
            let status = response.status();
            response.into_body().collect().await.unwrap();
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {source} => {destination:?}"
            );
            assert_eq!(
                fs::records(),
                before,
                "{method} {source} mutated records before refusing root"
            );
            assert_eq!(fs::commits(), 0);
        }
    }
}
