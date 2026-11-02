//! Deterministic HTTP/1.1 scripts; no external services or extra test crates.
#![allow(dead_code)]
use crate::path::{Path, PathBuf};
use alloc::{
    borrow::ToOwned,
    collections::VecDeque,
    format,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    thread::{self, JoinHandle},
    time::Instant,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
pub struct Directory(PathBuf);
impl Directory {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "trauma-regression-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn file(&self) -> PathBuf {
        self.0.join("fixture.bin")
    }
    pub fn checkpoint(&self) -> PathBuf {
        self.0.join("fixture.bin.trauma-resume")
    }
    pub fn bytes(&self) -> Vec<u8> {
        fs::read(self.file()).unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
}
impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}
pub struct Reply {
    method: &'static str,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl Reply {
    pub fn new(method: &'static str, status: u16) -> Self {
        Self {
            method,
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
    pub fn header(mut self, key: &str, value: impl ToString) -> Self {
        self.headers.push((key.to_owned(), value.to_string()));
        self
    }
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }
    pub fn metadata(method: &'static str, total: usize, etag: &str) -> Self {
        Self::new(method, 200)
            .header("Content-Length", total)
            .header("ETag", etag)
            .header("Accept-Ranges", "bytes")
    }
    pub fn partial(total: usize, body: impl Into<Vec<u8>>) -> Self {
        Self::metadata("GET", total, "\"v1\"").body(body)
    }
}

pub struct Server {
    pub url: String,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Request>>>,
    worker: Option<JoinHandle<usize>>,
}
impl Server {
    pub fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/fixture.bin", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (server_stop, log) = (stop.clone(), requests.clone());
        let worker = thread::spawn(move || {
            let mut replies = VecDeque::from(replies);
            let deadline = Instant::now() + Duration::from_secs(20);
            while !server_stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_request(&mut stream);
                        let reply = replies.pop_front().expect("unexpected extra HTTP request");
                        assert_eq!(
                            request.method, reply.method,
                            "unexpected request {request:?}"
                        );
                        log.lock().unwrap().push(request);
                        write!(
                            stream,
                            "HTTP/1.1 {} Fixture\r\nConnection: close\r\n",
                            reply.status
                        )
                        .unwrap();
                        for (key, value) in reply.headers {
                            write!(stream, "{key}: {value}\r\n").unwrap();
                        }
                        stream.write_all(b"\r\n").unwrap();
                        stream.write_all(&reply.body).unwrap();
                        stream.flush().unwrap();
                        // Give the client a chance to consume a short body before
                        // EOF; its advertised length intentionally remains larger.
                        if !reply.body.is_empty() {
                            thread::sleep(Duration::from_millis(5));
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(e) => panic!("HTTP accept: {e}"),
                }
            }
            replies.len()
        });
        Self {
            url,
            stop,
            requests,
            worker: Some(worker),
        }
    }
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
    pub fn finish(mut self) -> Vec<Request> {
        self.stop.store(true, Ordering::Relaxed);
        assert_eq!(
            self.worker.take().unwrap().join().unwrap(),
            0,
            "unconsumed HTTP responses"
        );
        self.requests()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn read_request(stream: &mut TcpStream) -> Request {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut data = Vec::new();
    while !data.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        let mut buffer = [0; 1024];
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0 && data.len() < 32 * 1024, "invalid request");
        data.extend_from_slice(&buffer[..count]);
    }
    let text = String::from_utf8(data).unwrap();
    let mut lines = text.lines();
    let mut words = lines.next().unwrap().split_whitespace();
    Request {
        method: words.next().unwrap().to_owned(),
        target: words.next().unwrap().to_owned(),
        headers: lines
            .filter_map(|line| {
                line.split_once(':')
                    .map(|(key, value)| (key.to_owned(), value.trim().to_owned()))
            })
            .collect(),
    }
}
