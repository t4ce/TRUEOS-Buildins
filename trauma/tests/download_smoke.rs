//! Loopback coverage for the transfer/retry path retained by the lean build.
use core::{
    error::Error as _,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Instant,
};
use trauma::{download::Download, download::Status, downloader::Downloader};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

#[test]
fn fresh_download_retries_and_writes_exact_bytes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let body = b"trauma loopback body\n";
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut requests = Vec::new();
        while requests.len() < 3 {
            assert!(Instant::now() < deadline, "missing expected HTTP request");
            let (mut socket, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(error) => panic!("accept: {error}"),
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).unwrap();
                assert!(count > 0, "unexpected EOF in request headers");
                request.extend_from_slice(&buffer[..count]);
                assert!(request.len() < 16 * 1024);
            }
            let request = String::from_utf8(request).unwrap();
            let method = request.split_whitespace().next().unwrap().to_owned();
            requests.push(method.clone());
            if requests.len() == 1 {
                socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            } else {
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                if method == "GET" {
                    socket.write_all(body).unwrap();
                }
            }
        }
        requests
    });

    let directory = std::env::temp_dir().join(format!(
        "trauma-lean-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let download = Download::builder()
        .url(format!("http://{address}/fixture.bin"))
        .unwrap()
        .build();
    let downloader = Downloader::builder()
        .directory(directory.clone())
        .retries(1)
        .resumable(false)
        .build();
    let summaries = runtime.block_on(downloader.download(&[download]));
    // The downloader must flush before returning; inspect while the runtime lives.
    let requests = server.join().unwrap();
    let output = fs::read(directory.join("fixture.bin"));
    fs::remove_dir_all(directory).unwrap();

    assert_eq!(requests, ["HEAD", "HEAD", "GET"]);
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].status(), &Status::Success);
    assert_eq!(summaries[0].statuscode(), reqwest::StatusCode::OK);
    assert_eq!(summaries[0].size(), body.len() as u64);
    assert_eq!(output.unwrap(), body);
}

#[test]
fn errors_keep_their_display_and_source_contract() {
    let error = trauma::Error::from(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "fixture denial",
    ));
    assert_eq!(error.to_string(), "I/O error");
    assert_eq!(error.source().unwrap().to_string(), "fixture denial");
    let error = trauma::Error::InvalidUrl("fixture".into());
    assert_eq!(error.to_string(), "Invalid URL: fixture");
    assert!(error.source().is_none());
}
