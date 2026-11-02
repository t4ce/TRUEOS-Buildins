//! Headless, incremental Hull download worker. The kernel owns queue admission,
//! generation authentication, crash recovery, and atomic drained retirement.
use alloc::string::{String, ToString};

/// Shared disk-relative location for Hull downloads and resume checkpoints.
pub const DOWNLOAD_DIRECTORY: &str = "common/hulldownloads";

/// Recognize a worker launch without acquiring an interactive terminal lease.
/// An explicit malformed worker directive fails closed instead of opening UI.
pub fn launch_generation(script: &[u8]) -> Result<Option<u64>, String> {
    let script =
        core::str::from_utf8(script).map_err(|_| "launch script is not UTF-8".to_string())?;
    let mut generation = None;
    for line in script.lines() {
        let mut words = line.split_whitespace();
        if words.next() != Some("download-worker") {
            continue;
        }
        if generation.is_some() {
            return Err("duplicate download-worker directive".to_string());
        }
        let value = words
            .next()
            .ok_or_else(|| "missing worker generation".to_string())?;
        if !value.bytes().all(|b| b.is_ascii_digit()) || words.next().is_some() {
            return Err("invalid worker generation".to_string());
        }
        let value: u64 = value
            .parse()
            .map_err(|_| "invalid worker generation".to_string())?;
        if value == 0 {
            return Err("invalid worker generation".to_string());
        }
        generation = Some(value);
    }
    Ok(generation)
}

#[cfg(any(target_os = "trueos", target_os = "zkvm"))]
pub use executor::run;

#[cfg(any(target_os = "trueos", target_os = "zkvm", test))]
mod executor {
    use super::*;
    use crate::{
        checkpoint::fingerprint,
        download::{Download, Status},
    };
    use alloc::{
        collections::{BTreeSet, VecDeque},
        format,
        string::{String, ToString},
        vec::Vec,
    };
    use core::{future::Future, time::Duration};
    use futures_util::{
        future::{select, Either},
        stream::FuturesUnordered,
        StreamExt,
    };

    const CAPACITY: usize = 32;
    const POLL_INTERVAL: Duration = Duration::from_millis(25);

    struct Job {
        id: u64,
        payload: Vec<u8>,
        received: bool,
    }
    enum Message {
        Drained,
        Wait,
        Job(Job),
    }
    fn decode(bytes: Vec<u8>) -> Result<Message, String> {
        match bytes.as_slice() {
            b"D" => Ok(Message::Drained),
            b"W" => Ok(Message::Wait),
            [kind @ (b'J' | b'P'), rest @ ..] if rest.len() >= 8 => {
                let id = u64::from_le_bytes(rest[..8].try_into().expect("job id length"));
                if id == 0 {
                    return Err("invalid broker job id".to_string());
                }
                Ok(Message::Job(Job {
                    id,
                    payload: rest[8..].to_vec(),
                    received: *kind == b'P',
                }))
            }
            _ => Err("invalid download broker response".to_string()),
        }
    }

    fn planned(job: Job) -> Result<Planned, String> {
        let separator = job
            .payload
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| "missing filename separator".to_string())?;
        let (url, mut hint) = (&job.payload[..separator], &job.payload[separator + 1..]);
        let received = if job.received {
            let split = hint
                .iter()
                .position(|byte| *byte == 0)
                .ok_or_else(|| "missing received-body separator".to_string())?;
            let body = hint[split + 1..].to_vec();
            if body.len() > 4 * 1024 * 1024 {
                return Err("received body too large".to_string());
            }
            hint = &hint[..split];
            Some(body)
        } else {
            None
        };
        let text = core::str::from_utf8(url).map_err(|_| "job URL is not UTF-8".to_string())?;
        let hint =
            core::str::from_utf8(hint).map_err(|_| "filename hint is not UTF-8".to_string())?;
        let mut url = reqwest::Url::parse(text).map_err(|_| "invalid download URL".to_string())?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err("worker only accepts HTTP(S) URLs".to_string());
        }
        url.set_fragment(None);
        let suggested = if hint.is_empty() {
            url.path_segments()
                .and_then(|mut segments| segments.next_back())
                .unwrap_or("")
        } else {
            hint
        };
        let name = safe_name(suggested, &url);
        let download = Download::builder()
            .url(url)
            .map_err(|e| e.to_string())?
            .filename_override(name.clone())
            .build();
        Ok(Planned {
            id: job.id,
            name,
            download,
            received,
        })
    }

    fn safe_name(hint: &str, url: &reqwest::Url) -> String {
        // Treat either platform's separators as untrusted input. The restricted
        // ASCII alphabet excludes drive prefixes, control characters and devices.
        let basename = hint.rsplit(['/', '\\']).next().unwrap_or("");
        let mut name: String = basename
            .chars()
            .take(120)
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        if name.trim_matches('.').is_empty() {
            name = "download".to_string();
        }
        let hash = fingerprint(url);
        // Keep the extension useful while disambiguating all URLs on every run,
        // not only collisions currently visible inside one worker generation.
        match name.rsplit_once('.').filter(|(stem, extension)| {
            !stem.is_empty()
                && !extension.is_empty()
                && extension.len() <= 16
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }) {
            Some((stem, extension)) => format!("{stem}--{hash}.{extension}"),
            None => format!("{name}--{hash}"),
        }
    }
    struct Planned {
        id: u64,
        name: String,
        download: Download,
        received: Option<Vec<u8>>,
    }
    struct Completion {
        id: u64,
        name: Option<String>,
        outcome: Status,
    }
    trait Broker {
        async fn next(&mut self) -> Result<Vec<u8>, String>;
        async fn done(&mut self, id: u64) -> Result<(), String>;
    }

    // Drive active transfers while broker I/O is pending. A queue read may have
    // atomically claimed a job, so it must never be cancelled by a winning download.
    async fn alongside<F, R>(
        io: F,
        running: &mut FuturesUnordered<R>,
    ) -> (F::Output, Vec<Completion>)
    where
        F: Future,
        R: Future<Output = Completion>,
    {
        let mut io = alloc::boxed::Box::pin(io);
        let mut completed = Vec::new();
        loop {
            if running.is_empty() {
                return (io.await, completed);
            }
            match select(io, running.next()).await {
                Either::Left((result, _)) => return (result, completed),
                Either::Right((Some(result), pending_io)) => {
                    completed.push(result);
                    io = pending_io;
                }
                Either::Right((None, pending_io)) => {
                    io = pending_io;
                }
            }
        }
    }

    async fn execute<B, A, F>(broker: &mut B, mut action: A) -> Result<(), String>
    where
        B: Broker,
        A: FnMut(Download, Option<Vec<u8>>) -> F,
        F: Future<Output = Status>,
    {
        let mut running = FuturesUnordered::new();
        let mut pending: VecDeque<Planned> = VecDeque::new();
        let mut completed: VecDeque<Completion> = VecDeque::new();
        let mut names = BTreeSet::new();
        let mut claimed = BTreeSet::new();
        loop {
            // Acknowledgments are terminal for successful and failed transfers.
            // Retry after a crashed worker is the kernel observer's responsibility.
            while let Some(job) = completed.pop_front() {
                let (result, more) = alongside(broker.done(job.id), &mut running).await;
                result?;
                match &job.outcome {
                    Status::Success => println!("Trauma: completed download {}", job.id),
                    Status::Skipped(_) => {
                        println!("Trauma: download {} already saved and verified", job.id)
                    }
                    Status::Fail(error) => println!("Trauma: failed download {}: {error}", job.id),
                    Status::NotStarted => {
                        println!("Trauma: failed download {}: transfer did not start", job.id)
                    }
                }
                claimed.remove(&job.id);
                if let Some(name) = job.name {
                    names.remove(&name);
                }
                completed.extend(more);
            }
            for _ in 0..pending.len() {
                let job = pending.pop_front().expect("pending length");
                if names.insert(job.name.clone()) {
                    let future = action(job.download, job.received);
                    running.push(async move {
                        let outcome = future.await;
                        Completion {
                            id: job.id,
                            name: Some(job.name),
                            outcome,
                        }
                    });
                } else {
                    pending.push_back(job);
                }
            }
            if claimed.len() < CAPACITY {
                let (response, finished) = alongside(broker.next(), &mut running).await;
                completed.extend(finished);
                match decode(response?)? {
                    Message::Drained => {
                        if claimed.is_empty() && pending.is_empty() && running.is_empty() {
                            println!("Trauma: download queue drained; worker retired");
                            return Ok(());
                        }
                        return Err("broker retired a generation with unfinished jobs".to_string());
                    }
                    Message::Wait => {
                        // W can also occur briefly before the launch observer binds
                        // the owner VM, including when no local transfer exists yet.
                        if completed.is_empty() {
                            let (_, finished) =
                                alongside(tokio::time::sleep(POLL_INTERVAL), &mut running).await;
                            completed.extend(finished);
                        }
                    }
                    Message::Job(job) => {
                        let id = job.id;
                        if !claimed.insert(id) {
                            return Err("broker repeated an in-flight job id".to_string());
                        }
                        match planned(job) {
                            Ok(job) => {
                                println!("Trauma: accepted download {id}: {}", job.name);
                                pending.push_back(job);
                            }
                            Err(error) => {
                                println!("Trauma: accepted download {id}");
                                completed.push_back(Completion {
                                    id,
                                    name: None,
                                    outcome: Status::Fail(error),
                                });
                            }
                        }
                    }
                }
            } else {
                let job = running
                    .next()
                    .await
                    .ok_or_else(|| "blocked worker without an active transfer".to_string())?;
                completed.push_back(job);
            }
        }
    }

    /// Run an authenticated kernel worker generation without terminal ownership.
    #[cfg(any(target_os = "trueos", target_os = "zkvm"))]
    pub async fn run(generation: u64) -> Result<(), String> {
        use crate::downloader::Downloader;
        struct KernelBroker {
            next: String,
            done: String,
        }
        impl Broker for KernelBroker {
            async fn next(&mut self) -> Result<Vec<u8>, String> {
                trueos::async_fs::read_file(self.next.as_bytes())
                    .await
                    .map_err(|e| format!("download queue read: {e:?}"))
            }
            async fn done(&mut self, id: u64) -> Result<(), String> {
                trueos::async_fs::write_file(self.done.as_bytes(), &id.to_le_bytes())
                    .await
                    .map_err(|e| format!("download acknowledgment: {e:?}"))
            }
        }
        let prepared = Downloader::builder()
            .directory(DOWNLOAD_DIRECTORY)
            .build()
            .prepare()
            .map_err(|e| e.to_string())?;
        println!("Trauma: Hull download worker {generation}; saving to {DOWNLOAD_DIRECTORY}");
        let mut broker = KernelBroker {
            next: format!("vFile:hulldownloads/next/{generation}"),
            done: format!("vFile:hulldownloads/done/{generation}"),
        };
        execute(&mut broker, |download, received| {
            let prepared = &prepared;
            async move {
                if let Some(bytes) = received {
                    crate::downloader::Downloader::builder()
                        .directory(DOWNLOAD_DIRECTORY)
                        .build()
                        .save_received(&download, &bytes)
                        .await
                } else {
                    prepared.download_one(&download).await.status().clone()
                }
            }
        })
        .await
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn received_body_is_saved_without_any_http_request() {
            use crate::{
                downloader::Downloader,
                test_support::{Directory, Server},
            };
            let server = Server::new(vec![]);
            let directory = Directory::new();
            let mut message = frame(1, &server.url, "archive.zip");
            message[0] = b'P';
            message.push(0);
            message.extend_from_slice(b"PK\x03\x04saved body");
            let Message::Job(job) = decode(message).unwrap() else {
                panic!()
            };
            let job = planned(job).unwrap();
            runtime().block_on(async {
                let downloader = Downloader::builder().directory(directory.path()).build();
                assert!(matches!(
                    downloader
                        .save_received(&job.download, job.received.as_ref().unwrap())
                        .await,
                    Status::Success
                ));
                assert_eq!(
                    tokio::fs::read(directory.path().join(&job.name))
                        .await
                        .unwrap(),
                    b"PK\x03\x04saved body"
                );
            });
            assert!(server.requests().is_empty());
        }

        use alloc::{rc::Rc, vec};
        use core::cell::RefCell;
        fn runtime() -> tokio::runtime::Runtime {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
        }
        fn frame(id: u64, url: &str, hint: &str) -> Vec<u8> {
            let mut frame = vec![b'J'];
            frame.extend_from_slice(&id.to_le_bytes());
            frame.extend_from_slice(url.as_bytes());
            frame.push(0);
            frame.extend_from_slice(hint.as_bytes());
            frame
        }
        #[derive(Default)]
        struct State {
            queue: VecDeque<Vec<u8>>,
            leased: BTreeSet<u64>,
            done: Vec<u64>,
            active: BTreeSet<String>,
            max_active: usize,
            max_claimed: usize,
            starts: usize,
            late_started_concurrently: bool,
            inject_late: bool,
            cancelled_io: bool,
        }
        struct FakeBroker {
            state: Rc<RefCell<State>>,
            startup_waits: usize,
            fail_done: bool,
            delay: bool,
        }
        struct IoGuard {
            state: Rc<RefCell<State>>,
            returned: bool,
        }
        impl Drop for IoGuard {
            fn drop(&mut self) {
                if !self.returned {
                    self.state.borrow_mut().cancelled_io = true;
                }
            }
        }
        impl Broker for FakeBroker {
            async fn next(&mut self) -> Result<Vec<u8>, String> {
                if self.startup_waits > 0 {
                    self.startup_waits -= 1;
                    return Ok(vec![b'W']);
                }
                let response = {
                    let mut state = self.state.borrow_mut();
                    if let Some(frame) = state.queue.pop_front() {
                        if let Ok(Message::Job(job)) = decode(frame.clone()) {
                            assert!(state.leased.insert(job.id));
                        }
                        state.max_claimed = state.max_claimed.max(state.leased.len());
                        frame
                    } else if state.leased.is_empty() {
                        vec![b'D']
                    } else {
                        vec![b'W']
                    }
                };
                let mut guard = IoGuard {
                    state: self.state.clone(),
                    returned: false,
                };
                if self.delay {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                guard.returned = true;
                Ok(response)
            }
            async fn done(&mut self, id: u64) -> Result<(), String> {
                if self.fail_done {
                    return Err("injected broker failure".to_string());
                }
                let mut guard = IoGuard {
                    state: self.state.clone(),
                    returned: false,
                };
                if self.delay {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                let mut state = self.state.borrow_mut();
                assert!(state.leased.remove(&id));
                assert!(!state.done.contains(&id));
                state.done.push(id);
                guard.returned = true;
                Ok(())
            }
        }
        #[test]
        fn dynamic_jobs_are_bounded_serialized_and_never_cancel_claiming_io() {
            runtime().block_on(async {
                let state = Rc::new(RefCell::new(State {
                    inject_late: true,
                    ..State::default()
                }));
                for id in 1..=80 {
                    // Two identical destinations must be serialized; other URLs
                    // sharing the suggested name receive stable distinct hashes.
                    let url = if id == 2 {
                        "http://fixture.test/1".to_string()
                    } else {
                        format!("http://fixture.test/{id}")
                    };
                    state
                        .borrow_mut()
                        .queue
                        .push_back(frame(id, &url, "same.bin"));
                }
                let mut broker = FakeBroker {
                    state: state.clone(),
                    startup_waits: 1,
                    fail_done: false,
                    delay: true,
                };
                execute(&mut broker, |download, received| {
                    assert!(received.is_none());
                    let state = state.clone();
                    async move {
                        let name = download.filename_override().unwrap().clone();
                        let late = download.url_as_str().ends_with("/late");
                        {
                            let mut state = state.borrow_mut();
                            if late {
                                state.late_started_concurrently = !state.active.is_empty();
                            }
                            assert!(
                                state.active.insert(name.clone()),
                                "conflicting active writers"
                            );
                            state.max_active = state.max_active.max(state.active.len());
                            state.starts += 1;
                            if state.inject_late {
                                state.inject_late = false;
                                state.queue.push_front(frame(
                                    1000,
                                    "http://fixture.test/late",
                                    "same.bin",
                                ));
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        assert!(state.borrow_mut().active.remove(&name));
                        Status::Success
                    }
                })
                .await
                .unwrap();
                let state = state.borrow();
                assert_eq!(state.starts, 81);
                assert_eq!(state.done.len(), 81);
                assert!(state.max_active > 1 && state.max_active <= CAPACITY);
                assert_eq!(state.max_claimed, CAPACITY);
                assert!(state.late_started_concurrently);
                assert!(!state.cancelled_io);
                assert!(state.active.is_empty() && state.leased.is_empty());
            });
        }
        #[test]
        fn invalid_payloads_are_acknowledged_without_execution_and_broker_failure_propagates() {
            runtime().block_on(async {
                let state = Rc::new(RefCell::new(State::default()));
                state.borrow_mut().queue.extend([
                    frame(1, "file:///secret", "../escape"),
                    frame(2, "not a URL", "x"),
                    frame(3, "http://fixture.test/ok", "ok"),
                    [
                        vec![b'J'],
                        4u64.to_le_bytes().to_vec(),
                        b"missing separator".to_vec(),
                    ]
                    .concat(),
                ]);
                let mut broker = FakeBroker {
                    state: state.clone(),
                    startup_waits: 0,
                    fail_done: false,
                    delay: false,
                };
                execute(&mut broker, |_, received| {
                    assert!(received.is_none());
                    state.borrow_mut().starts += 1;
                    async { Status::Success }
                })
                .await
                .unwrap();
                assert_eq!(state.borrow().starts, 1);
                assert_eq!(state.borrow().done.len(), 4);
                state
                    .borrow_mut()
                    .queue
                    .push_back(frame(5, "http://fixture.test/ok", "ok"));
                broker.fail_done = true;
                assert!(execute(&mut broker, |_, _| async { Status::Success })
                    .await
                    .is_err());
                assert!(state.borrow().leased.contains(&5)); // observer can requeue
            });
        }
        #[test]
        fn launch_and_protocol_validation_fail_closed() {
            assert_eq!(launch_generation(b""), Ok(None));
            assert_eq!(
                launch_generation(b"fs-scope trueosfs\ndownload-worker 42\n"),
                Ok(Some(42))
            );
            for script in [
                b"download-worker".as_slice(),
                b"download-worker 0",
                b"download-worker -1",
                b"download-worker 2 extra",
                b"download-worker 18446744073709551616",
                b"download-worker 1\ndownload-worker 2",
                b"\xff",
            ] {
                assert!(launch_generation(script).is_err());
            }
            for frame in [b"".as_slice(), b"DX", b"Jshort", b"X", b"J\0\0\0\0\0\0\0\0"] {
                assert!(decode(frame.to_vec()).is_err());
            }
        }
        #[test]
        fn filename_hints_cannot_escape_or_conflict_across_urls() {
            let url = reqwest::Url::parse("https://fixture.test/file?token=secret").unwrap();
            for hint in [
                "../../escape.txt",
                "C:\\windows\\secret.bin",
                "..",
                "",
                "/absolute",
                "x\n\0:y",
                "💣/文件.bin",
                &"a".repeat(400),
            ] {
                let name = safe_name(hint, &url);
                assert!(name.len() < 200);
                assert!(name.is_ascii());
                assert!(!name.contains(['/', '\\', ':', '\0', '\n']));
                assert_eq!(crate::path::Path::new(&name).components().count(), 1);
                assert!(name.contains(&fingerprint(&url)));
            }
            assert_ne!(
                safe_name("foo.trauma-resume", &url),
                format!("{}.trauma-resume", safe_name("foo", &url))
            );
            assert_eq!(safe_name("SAME.BIN", &url), safe_name("same.bin", &url));
            let other = reqwest::Url::parse("https://other.test/file?token=secret").unwrap();
            assert_ne!(safe_name("same.bin", &url), safe_name("same.bin", &other));
            let a = planned(Job {
                id: 1,
                payload: b"https://fixture.test/a#one\0same.bin".to_vec(),
                received: false,
            })
            .unwrap();
            let b = planned(Job {
                id: 2,
                payload: b"https://fixture.test/a#two\0same.bin".to_vec(),
                received: false,
            })
            .unwrap();
            assert_eq!(a.name, b.name);
            assert_eq!(a.download.url_as_str(), "https://fixture.test/a");
        }
    }
}
