//! Run actual native adapters against deterministic native services.
#![allow(dead_code, unused_imports)]
extern crate alloc;
extern crate self as trueos;
mod body {
    include!(concat!(env!("OUT_DIR"), "/body.rs"));
}
mod checkpoint {
    include!(concat!(env!("OUT_DIR"), "/checkpoint.rs"));
}
pub mod path {
    pub use trueos_path::{Path, PathBuf};
}
#[path = "../../src/native_http.rs"]
mod native_http;
#[path = "../../src/native_storage.rs"]
mod storage;
mod transport {
    pub use http::{header, HeaderMap};
    pub use url::Url;
}
mod transfer {
    pub struct TransferPlan {
        pub offset: u64,
        pub total: Option<u64>,
    }
}

pub mod async_fs {
    use sha2::{Digest, Sha256};
    use std::{cell::RefCell, collections::BTreeMap};
    pub const ERR_NOT_FOUND: i32 = -8;
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
    }
    pub struct TypedMetadata {
        pub kind: NodeKind,
        pub len: u64,
        pub content_type: ContentTypeId,
    }
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Record {
        pub bytes: Vec<u8>,
        pub content_type: ContentTypeId,
        pub file_id: u64,
        pub revision: u64,
        pub protected: bool,
    }
    #[derive(Default)]
    pub struct State {
        pub files: BTreeMap<String, Record>,
        pub events: Vec<(String, Vec<u8>)>,
        pub fail_write: bool,
        pub fail_path: Option<String>,
        pub race: Option<(String, Record)>,
        pub change_on_read: Option<(String, Record)>,
    }
    thread_local! { static FS:RefCell<State>=RefCell::new(State::default()); }
    pub fn state<R>(f: impl FnOnce(&mut State) -> R) -> R {
        FS.with(|fs| f(&mut fs.borrow_mut()))
    }
    pub fn reset() {
        state(|fs| *fs = State::default());
    }
    pub fn seed(path: &str, bytes: &[u8], content_type: ContentTypeId, protected: bool) {
        state(|fs| {
            fs.files.insert(
                path.into(),
                Record {
                    bytes: bytes.into(),
                    content_type,
                    file_id: 77,
                    revision: 1,
                    protected,
                },
            );
        });
    }
    fn key(path: &[u8]) -> &str {
        std::str::from_utf8(path).unwrap()
    }
    pub async fn metadata_v2(path: &[u8]) -> Result<MetadataV2, i32> {
        state(|fs| {
            let r = fs.files.get(key(path)).ok_or(ERR_NOT_FOUND)?;
            Ok(MetadataV2 {
                kind: NodeKind::File,
                len: r.bytes.len() as u64,
                file_id: r.file_id,
                root_id: 7,
                revision: r.revision,
            })
        })
    }
    pub async fn typed_metadata(path: &[u8]) -> Result<TypedMetadata, i32> {
        state(|fs| {
            let r = fs.files.get(key(path)).ok_or(ERR_NOT_FOUND)?;
            Ok(TypedMetadata {
                kind: NodeKind::File,
                len: r.bytes.len() as u64,
                content_type: r.content_type,
            })
        })
    }
    pub async fn read_file(path: &[u8]) -> Result<Vec<u8>, i32> {
        state(|fs| {
            let bytes = fs.files.get(key(path)).ok_or(ERR_NOT_FOUND)?.bytes.clone();
            if let Some((path, record)) = fs.change_on_read.take() {
                fs.files.insert(path, record);
            }
            Ok(bytes)
        })
    }
    pub async fn create_dir_all(_: &[u8]) -> Result<(), i32> {
        Ok(())
    }
    pub async fn remove_file(path: &[u8]) -> Result<(), i32> {
        state(|fs| fs.files.remove(key(path)).map(|_| ()).ok_or(ERR_NOT_FOUND))
    }
    fn publish(
        path: &[u8],
        bytes: &[u8],
        declared: ContentTypeId,
        create_new: bool,
        expected: Option<[u8; 32]>,
        reset_blob: bool,
    ) -> Result<(), i32> {
        state(|fs| {
            if let Some((path, record)) = fs.race.take() {
                fs.files.insert(path, record);
            }
            if fs.fail_write || fs.fail_path.as_deref() == Some(key(path)) {
                fs.fail_write = false;
                return Err(-5);
            }
            let old = fs.files.get(key(path));
            if create_new && old.is_some() {
                return Err(-9);
            }
            if let Some(expected) = expected {
                let old = old.ok_or(ERR_NOT_FOUND)?;
                if <[u8; 32]>::from(Sha256::digest(&old.bytes)) != expected {
                    return Err(-17);
                }
            }
            if reset_blob && old.is_none_or(|r| r.protected) {
                return Err(-6);
            }
            let content_type = if reset_blob {
                ContentTypeId::BLOB
            } else {
                old.map_or(declared, |r| r.content_type)
            };
            if content_type == ContentTypeId::UTF8_TEXT && std::str::from_utf8(bytes).is_err() {
                return Err(-6);
            }
            let record = Record {
                bytes: bytes.into(),
                content_type,
                file_id: old.map_or(91, |r| r.file_id),
                revision: old.map_or(1, |r| r.revision + 1),
                protected: old.is_some_and(|r| r.protected),
            };
            fs.files.insert(key(path).into(), record);
            fs.events.push((key(path).into(), bytes.into()));
            Ok(())
        })
    }
    pub async fn write_file_typed_checked(
        path: &[u8],
        bytes: &[u8],
        declared: ContentTypeId,
        create_new: bool,
        expected: Option<[u8; 32]>,
    ) -> Result<(), i32> {
        publish(path, bytes, declared, create_new, expected, false)
    }
    pub async fn reset_blob_checked(path: &[u8], expected: [u8; 32]) -> Result<(), i32> {
        publish(path, &[], ContentTypeId::BLOB, false, Some(expected), true)
    }
}

pub mod netfs {
    pub mod http {
        use std::{cell::RefCell, collections::VecDeque};
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum Method {
            Get,
            Head,
        }
        #[derive(Clone, Debug)]
        pub struct Header {
            pub name: String,
            pub value: Vec<u8>,
        }
        #[derive(Clone, Debug, Default)]
        pub struct Timeouts {
            pub connect_ms: u32,
            pub tls_ms: u32,
            pub read_idle_ms: u32,
        }
        #[derive(Clone, Debug)]
        pub struct RequestOwned {
            pub method: Method,
            pub url: String,
            pub headers: Vec<Header>,
            pub timeouts: Timeouts,
        }
        #[derive(Clone, Debug)]
        pub struct ResponseHead {
            pub status: u16,
            pub url: String,
            pub headers: Vec<Header>,
        }
        pub enum BodyRead {
            Pending,
            Data(usize),
            Eof,
        }
        pub enum Read {
            Pending,
            Data(Vec<u8>),
            Error(i32),
            Eof,
        }
        pub struct Script {
            pub head: Result<ResponseHead, i32>,
            pub reads: VecDeque<Read>,
        }
        impl Script {
            pub fn response(status: u16, headers: &[(&str, &[u8])]) -> Self {
                Self {
                    head: Ok(ResponseHead {
                        status,
                        url: "ignored.single-hop".into(),
                        headers: headers
                            .iter()
                            .map(|(k, v)| Header {
                                name: (*k).into(),
                                value: (*v).into(),
                            })
                            .collect(),
                    }),
                    reads: VecDeque::new(),
                }
            }
        }
        #[derive(Default)]
        pub struct State {
            pub scripts: VecDeque<Result<Script, i32>>,
            pub starts: Vec<RequestOwned>,
            pub active: usize,
            pub dropped: usize,
        }
        thread_local! {static NET:RefCell<State>=RefCell::new(State::default());}
        pub fn state<R>(f: impl FnOnce(&mut State) -> R) -> R {
            NET.with(|net| f(&mut net.borrow_mut()))
        }
        pub fn reset() {
            state(|net| *net = State::default())
        }
        pub struct Request {
            script: Script,
        }
        impl Request {
            pub fn start(spec: &RequestOwned) -> Result<Self, i32> {
                state(|net| {
                    net.starts.push(spec.clone());
                    let script = net.scripts.pop_front().expect("unexpected request")?;
                    net.active += 1;
                    Ok(Self { script })
                })
            }
            pub fn try_head(&mut self) -> Result<Option<ResponseHead>, i32> {
                self.script.head.clone().map(Some)
            }
            pub fn try_read(&mut self, out: &mut [u8]) -> Result<BodyRead, i32> {
                match self.script.reads.pop_front().unwrap_or(Read::Eof) {
                    Read::Pending => Ok(BodyRead::Pending),
                    Read::Eof => Ok(BodyRead::Eof),
                    Read::Error(e) => Err(e),
                    Read::Data(bytes) => {
                        out[..bytes.len()].copy_from_slice(&bytes);
                        Ok(BodyRead::Data(bytes.len()))
                    }
                }
            }
        }
        impl Drop for Request {
            fn drop(&mut self) {
                state(|net| {
                    net.active -= 1;
                    net.dropped += 1;
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_fs::{self as fs, ContentTypeId as Type};
    use body::Sink;
    use futures_util::StreamExt;
    use http::{header, HeaderMap, HeaderValue, Method};
    use netfs::http::{self as net, Read, Script};
    use path::Path;
    fn record(path: &str) -> fs::Record {
        fs::state(|s| s.files[path].clone())
    }
    fn path() -> &'static Path {
        Path::new("/download.bin")
    }
    fn checkpoint() -> checkpoint::Checkpoint {
        let url = url::Url::parse("https://user:password@fixture.test/file?secret=x").unwrap();
        checkpoint::Checkpoint::new(&url, &url, "\"v1\"".into(), Some(6))
    }
    fn client(retries: u32) -> native_http::Client {
        native_http::Client::new(HeaderMap::new(), retries)
    }
    fn url(value: &str) -> url::Url {
        url::Url::parse(value).unwrap()
    }
    fn script(status: u16, headers: &[(&str, &[u8])]) {
        net::state(|s| s.scripts.push_back(Ok(Script::response(status, headers))));
    }
    async fn send(method: Method) -> Result<native_http::Response, String> {
        client(0)
            .send(method, url("https://fixture.test/start"), HeaderMap::new())
            .await
    }

    #[tokio::test]
    async fn buffered_resume_advances_only_after_explicit_commit_and_drop_never_writes() {
        fs::reset();
        fs::seed("/download.bin", b"old", Type::BLOB, false);
        let old = record("/download.bin");
        let mut file = storage::open(path(), 3).await.unwrap();
        file.write_all(b"new").await.unwrap();
        assert_eq!(file.committed_len().await.unwrap(), 3);
        assert_eq!(record("/download.bin"), old);
        file.flush().await.unwrap();
        assert_eq!(file.committed_len().await.unwrap(), 6);
        assert_eq!(record("/download.bin").bytes, b"oldnew");
        assert_eq!(record("/download.bin").file_id, old.file_id);
        file.write_all(b"discard").await.unwrap();
        drop(file);
        assert_eq!(record("/download.bin").bytes, b"oldnew");
    }
    #[tokio::test]
    async fn failed_commit_keeps_previous_durable_length_bytes_and_checkpoint() {
        fs::reset();
        fs::seed("/download.bin", b"old", Type::BLOB, false);
        let saved = checkpoint();
        saved.save(path()).await.unwrap();
        let old = record("/download.bin.trauma-resume");
        let mut file = storage::open(path(), 3).await.unwrap();
        file.write_all(b"new").await.unwrap();
        fs::state(|s| s.fail_write = true);
        assert!(file.flush().await.is_err());
        assert_eq!(file.committed_len().await.unwrap(), 3);
        drop(file);
        assert_eq!(record("/download.bin").bytes, b"old");
        assert_eq!(record("/download.bin.trauma-resume"), old);
    }
    #[tokio::test]
    async fn restart_publishes_empty_blob_before_new_validator_checkpoint_and_preserves_identity() {
        fs::reset();
        fs::seed("/download.bin", b"old", Type::UTF8_TEXT, false);
        let old = record("/download.bin");
        let mut file = storage::open(path(), 0).await.unwrap();
        assert_eq!(file.committed_len().await.unwrap(), 0);
        assert_eq!(record("/download.bin").bytes, b"");
        assert_eq!(record("/download.bin").content_type, Type::BLOB);
        assert_eq!(record("/download.bin").file_id, old.file_id);
        checkpoint().save(path()).await.unwrap();
        let events = fs::state(|s| s.events.clone());
        assert_eq!(events[0], ("/download.bin".into(), vec![]));
        assert_eq!(events[1].0, "/download.bin.trauma-resume");
        assert!(events[1].1.ends_with(b"partial\n"));
        assert!(!String::from_utf8(events[1].1.clone())
            .unwrap()
            .contains("password"));
    }
    #[tokio::test]
    async fn protected_typed_restart_rejects_without_mutation() {
        fs::reset();
        fs::seed("/download.bin", b"protected", Type::UTF8_TEXT, true);
        let old = record("/download.bin");
        assert!(storage::open(path(), 0).await.is_err());
        assert_eq!(record("/download.bin"), old);
        assert!(fs::state(|s| s.events.is_empty()));
    }
    #[tokio::test]
    async fn concurrent_change_after_snapshot_rejects_prefix_publication() {
        fs::reset();
        fs::seed("/download.bin", b"old", Type::BLOB, false);
        let mut file = storage::open(path(), 3).await.unwrap();
        file.write_all(b"new").await.unwrap();
        let mut foreign = record("/download.bin");
        foreign.bytes = b"foreign".to_vec();
        foreign.revision += 1;
        foreign.protected = true;
        fs::state(|s| s.race = Some(("/download.bin".into(), foreign.clone())));
        assert!(file.flush().await.is_err());
        assert_eq!(file.committed_len().await.unwrap(), 3);
        assert_eq!(record("/download.bin"), foreign);
    }
    #[tokio::test]
    async fn create_only_restart_never_clobbers_a_racing_record() {
        fs::reset();
        fs::seed("/other", b"foreign", Type::UTF8_TEXT, true);
        let foreign = record("/other");
        fs::state(|s| s.race = Some(("/download.bin".into(), foreign.clone())));
        assert!(storage::open(path(), 0).await.is_err());
        assert_eq!(record("/download.bin"), foreign);
    }
    #[tokio::test]
    async fn snapshot_retries_when_same_length_revision_changes_while_reading() {
        fs::reset();
        fs::seed("/download.bin", b"old", Type::BLOB, false);
        let mut changed = record("/download.bin");
        changed.bytes = b"NEW".to_vec();
        changed.revision += 1;
        fs::state(|s| s.change_on_read = Some(("/download.bin".into(), changed)));
        let mut file = storage::open(path(), 3).await.unwrap();
        file.write_all(b"end").await.unwrap();
        file.flush().await.unwrap();
        assert_eq!(record("/download.bin").bytes, b"NEWend");
    }
    #[tokio::test]
    async fn late_body_error_commits_accepted_prefix_without_completion_checkpoint() {
        fs::reset();
        fs::seed("/download.bin", b"old", Type::BLOB, false);
        let saved = checkpoint();
        saved.save(path()).await.unwrap();
        let mut file = storage::open(path(), 3).await.unwrap();
        let chunks = futures_util::stream::iter([Ok(b"ne".to_vec()), Err("late HTTP failure")]);
        let result = body::save_body(
            chunks,
            &mut file,
            &transfer::TransferPlan {
                offset: 3,
                total: Some(6),
            },
            Some(saved),
            path(),
            |_| {},
        )
        .await;
        assert_eq!(result.size, 5);
        assert!(result.error.unwrap().contains("late HTTP"));
        assert_eq!(record("/download.bin").bytes, b"oldne");
        assert!(
            !checkpoint::Checkpoint::load(path())
                .await
                .unwrap()
                .unwrap()
                .complete
        );
    }
    #[tokio::test]
    async fn flush_failure_reports_only_old_durable_prefix_and_cannot_mark_complete() {
        fs::reset();
        fs::seed("/download.bin", b"old", Type::BLOB, false);
        let saved = checkpoint();
        saved.save(path()).await.unwrap();
        let mut file = storage::open(path(), 3).await.unwrap();
        fs::state(|s| s.fail_path = Some("/download.bin".into()));
        let result = body::save_body(
            futures_util::stream::iter([Ok::<_, String>(b"new".to_vec())]),
            &mut file,
            &transfer::TransferPlan {
                offset: 3,
                total: Some(6),
            },
            Some(saved),
            path(),
            |_| {},
        )
        .await;
        assert_eq!(result.size, 3);
        assert!(result.error.unwrap().contains("flush"));
        assert_eq!(record("/download.bin").bytes, b"old");
        assert!(
            !checkpoint::Checkpoint::load(path())
                .await
                .unwrap()
                .unwrap()
                .complete
        );
    }
    #[tokio::test]
    async fn completion_checkpoint_failure_keeps_committed_bytes_and_returns_failure() {
        fs::reset();
        fs::seed("/download.bin", b"old", Type::BLOB, false);
        let saved = checkpoint();
        saved.save(path()).await.unwrap();
        let mut file = storage::open(path(), 3).await.unwrap();
        fs::state(|s| s.fail_path = Some("/download.bin.trauma-resume".into()));
        let result = body::save_body(
            futures_util::stream::iter([Ok::<_, String>(b"new".to_vec())]),
            &mut file,
            &transfer::TransferPlan {
                offset: 3,
                total: Some(6),
            },
            Some(saved),
            path(),
            |_| {},
        )
        .await;
        assert_eq!(result.size, 6);
        assert!(result.error.is_some());
        assert_eq!(record("/download.bin").bytes, b"oldnew");
        assert!(
            !checkpoint::Checkpoint::load(path())
                .await
                .unwrap()
                .unwrap()
                .complete
        );
    }
    #[tokio::test]
    async fn complete_checkpoint_follows_successful_body_commit() {
        fs::reset();
        let saved = checkpoint();
        let mut file = storage::open(path(), 0).await.unwrap();
        saved.save(path()).await.unwrap();
        let result = body::save_body(
            futures_util::stream::iter([Ok::<_, String>(b"newnew".to_vec())]),
            &mut file,
            &transfer::TransferPlan {
                offset: 0,
                total: Some(6),
            },
            Some(saved),
            path(),
            |_| {},
        )
        .await;
        assert_eq!(result.size, 6);
        assert!(result.error.is_none());
        assert!(
            checkpoint::Checkpoint::load(path())
                .await
                .unwrap()
                .unwrap()
                .complete
        );
        let events = fs::state(|s| s.events.clone());
        assert_eq!(events[2], ("/download.bin".into(), b"newnew".to_vec()));
        assert!(events[3].1.ends_with(b"complete\n"));
    }
    #[tokio::test(start_paused = true)]
    async fn admission_pressure_waits_cooperatively_without_spending_retry_budget() {
        net::reset();
        net::state(|s| s.scripts.extend([Err(-3), Err(-3)]));
        script(200, &[]);
        let started = tokio::time::Instant::now();
        let response = send(Method::HEAD).await.unwrap();
        assert_eq!(response.status(), 200);
        assert!(started.elapsed() >= std::time::Duration::from_millis(4));
        net::state(|s| {
            assert_eq!(s.starts.len(), 3);
            assert_eq!(s.active, 0);
            assert_eq!(s.dropped, 1)
        });
    }
    #[tokio::test(start_paused = true)]
    async fn transient_prehead_failure_retries_then_head_releases_transport() {
        net::reset();
        net::state(|s| {
            s.scripts.push_back(Ok(Script {
                head: Err(-112),
                reads: Default::default(),
            }))
        });
        script(200, &[]);
        let response = client(1)
            .send(Method::HEAD, url("https://fixture.test/"), HeaderMap::new())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        net::state(|s| {
            assert_eq!(s.starts.len(), 2);
            assert_eq!(s.dropped, 2);
            assert_eq!(s.active, 0)
        });
    }
    #[tokio::test(start_paused = true)]
    async fn tls_protocol_and_invalid_start_are_fatal_without_retries() {
        for code in [-13, -115, -10, -4, -8] {
            net::reset();
            net::state(|s| s.scripts.push_back(Err(code)));
            assert!(client(3)
                .send(Method::GET, url("https://fixture.test/"), HeaderMap::new())
                .await
                .is_err());
            net::state(|s| assert_eq!(s.starts.len(), 1));
        }
    }
    #[tokio::test(start_paused = true)]
    async fn redirect_policy_failure_is_fatal_and_drops_exchange() {
        net::reset();
        script(302, &[("location", b"file:///outside")]);
        let error = client(3)
            .send(Method::GET, url("https://fixture.test/"), HeaderMap::new())
            .await
            .err()
            .unwrap();
        assert!(error.contains("HTTP(S)"));
        net::state(|s| {
            assert_eq!(s.starts.len(), 1);
            assert_eq!(s.dropped, 1);
            assert_eq!(s.active, 0)
        });
    }
    #[tokio::test]
    async fn relative_redirect_preserves_duplicate_headers_and_releases_old_handle() {
        net::reset();
        script(302, &[("location", b"../final")]);
        script(
            200,
            &[
                ("etag", b"\"a\""),
                ("etag", b"\"b\""),
                ("x-opaque", b"\x80"),
            ],
        );
        let response = client(0)
            .send(
                Method::GET,
                url("https://fixture.test/a/start#fragment"),
                HeaderMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(response.url().as_str(), "https://fixture.test/final");
        assert_eq!(response.headers().get_all("etag").iter().count(), 2);
        assert_eq!(response.headers()["x-opaque"].as_bytes(), b"\x80");
        net::state(|s| {
            assert_eq!(s.dropped, 1);
            assert_eq!(s.active, 1);
            assert_eq!(s.starts[0].timeouts.connect_ms, 0);
            assert_eq!(s.starts[0].timeouts.tls_ms, 0);
            assert_eq!(s.starts[0].timeouts.read_idle_ms, 0);
            assert!(s.starts[0]
                .headers
                .iter()
                .any(|h| h.name == "accept-encoding" && h.value == b"identity"))
        });
        drop(response);
        net::state(|s| assert_eq!(s.active, 0));
    }
    #[tokio::test]
    async fn cross_origin_redirect_strips_credentials_and_cookie_including_https_to_http() {
        net::reset();
        script(302, &[("location", b"http://other.test/final")]);
        script(200, &[]);
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_static("secret=x"));
        let client = native_http::Client::new(headers, 0);
        let response = client
            .send(
                Method::HEAD,
                url("https://user:p%FFss@fixture.test/start"),
                HeaderMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(response.url().as_str(), "http://other.test/final");
        net::state(|s| {
            assert!(!s.starts[0].url.contains("user"));
            assert!(s.starts[0]
                .headers
                .iter()
                .any(|h| h.name == "authorization" && h.value == b"Basic dXNlcjpw/3Nz"));
            assert!(!s.starts[1]
                .headers
                .iter()
                .any(|h| h.name == "authorization" || h.name == "cookie"));
        });
    }
    #[tokio::test]
    async fn same_origin_redirect_keeps_auth_and_duplicate_location_rejects() {
        net::reset();
        script(302, &[("location", b"/final")]);
        script(200, &[]);
        let response = client(0)
            .send(
                Method::HEAD,
                url("https://u:p@fixture.test/start"),
                HeaderMap::new(),
            )
            .await
            .unwrap();
        net::state(|s| {
            assert!(s.starts[1]
                .headers
                .iter()
                .any(|h| h.name == "authorization"))
        });
        drop(response);
        net::reset();
        script(302, &[("location", b"/a"), ("location", b"/b")]);
        assert!(send(Method::GET).await.is_err());
        net::state(|s| assert_eq!(s.active, 0));
    }
    #[tokio::test]
    async fn late_transport_error_yields_prefix_once_then_cancels_without_replay() {
        net::reset();
        let mut reply = Script::response(200, &[]);
        reply.reads.extend([
            Read::Pending,
            Read::Data(b"prefix".to_vec()),
            Read::Error(-114),
        ]);
        net::state(|s| s.scripts.push_back(Ok(reply)));
        let response = client(3)
            .send(Method::GET, url("https://fixture.test/"), HeaderMap::new())
            .await
            .unwrap();
        let stream = response.bytes_stream();
        futures_util::pin_mut!(stream);
        assert_eq!(stream.next().await.unwrap().unwrap(), b"prefix");
        assert!(stream.next().await.unwrap().unwrap_err().contains("-114"));
        assert!(stream.next().await.is_none());
        net::state(|s| {
            assert_eq!(s.starts.len(), 1);
            assert_eq!(s.active, 0);
            assert_eq!(s.dropped, 1)
        });
    }
    #[tokio::test]
    async fn dropping_body_pending_poll_cancels_owned_transport() {
        net::reset();
        let mut reply = Script::response(200, &[]);
        reply.reads.push_back(Read::Pending);
        net::state(|s| s.scripts.push_back(Ok(reply)));
        let response = send(Method::GET).await.unwrap();
        let mut stream = Box::pin(response.bytes_stream());
        let next = stream.next();
        futures_util::pin_mut!(next);
        assert!(futures_util::poll!(next).is_pending());
        drop(stream);
        net::state(|s| assert_eq!(s.active, 0));
    }
    #[tokio::test]
    async fn oversized_chunk_rejects_whole_chunk_and_reports_only_committed_prefix() {
        fs::reset();
        fs::seed("/download.bin", b"old", Type::BLOB, false);
        let saved = checkpoint();
        saved.save(path()).await.unwrap();
        let mut file = storage::open(path(), 3).await.unwrap();
        let chunks = futures_util::stream::iter([Ok::<_, String>(b"too big".to_vec())]);
        let result = body::save_body(
            chunks,
            &mut file,
            &transfer::TransferPlan {
                offset: 3,
                total: Some(6),
            },
            Some(saved),
            path(),
            |_| {},
        )
        .await;
        assert_eq!(result.size, 3);
        assert!(result.error.unwrap().contains("exceeds"));
        assert_eq!(record("/download.bin").bytes, b"old");
        assert!(
            !checkpoint::Checkpoint::load(path())
                .await
                .unwrap()
                .unwrap()
                .complete
        );
    }
    #[tokio::test]
    async fn short_body_commits_exact_received_prefix_but_never_completes() {
        fs::reset();
        fs::seed("/download.bin", b"old", Type::BLOB, false);
        let saved = checkpoint();
        saved.save(path()).await.unwrap();
        let mut file = storage::open(path(), 3).await.unwrap();
        let result = body::save_body(
            futures_util::stream::iter([Ok::<_, String>(b"n".to_vec())]),
            &mut file,
            &transfer::TransferPlan {
                offset: 3,
                total: Some(6),
            },
            Some(saved),
            path(),
            |_| {},
        )
        .await;
        assert_eq!(result.size, 4);
        assert!(result.error.unwrap().contains("before"));
        assert_eq!(record("/download.bin").bytes, b"oldn");
        assert!(
            !checkpoint::Checkpoint::load(path())
                .await
                .unwrap()
                .unwrap()
                .complete
        );
    }
    #[tokio::test]
    async fn redirect_referer_is_sanitized_and_removed_on_https_downgrade() {
        net::reset();
        script(302, &[("location", b"/middle")]);
        script(302, &[("location", b"http://other.test/final")]);
        script(200, &[]);
        let mut headers = HeaderMap::new();
        headers.insert("cookie2", HeaderValue::from_static("secret=legacy"));
        let client = native_http::Client::new(headers, 0);
        client
            .send(
                Method::HEAD,
                url("https://user:password@fixture.test/start#private"),
                HeaderMap::new(),
            )
            .await
            .unwrap();
        net::state(|s| {
            let referer = s.starts[1]
                .headers
                .iter()
                .find(|h| h.name == "referer")
                .unwrap();
            assert_eq!(referer.value, b"https://fixture.test/start");
            assert!(s.starts[1].headers.iter().any(|h| h.name == "cookie2"));
            assert!(!s.starts[2]
                .headers
                .iter()
                .any(|h| h.name == "cookie2" || h.name == "referer"));
            assert_eq!(s.active, 0);
        });
    }
}
