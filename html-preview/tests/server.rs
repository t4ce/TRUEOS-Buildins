//! Exercise the real standalone process, including startup and folder updates.
#![cfg(not(target_os = "trueos"))]
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use trueos_ide::html_preview::{CSS, Config, HTML, JS, PROTOCOL, Ready};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn request(port: u16, method: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut result = String::new();
    stream.read_to_string(&mut result).unwrap();
    result
}

#[test]
fn standalone_startup_and_save_refresh_use_the_same_server() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("public");
    std::fs::create_dir(&root).unwrap();
    for (name, text) in [("index.html", HTML), ("main.js", JS), ("style.css", CSS)] {
        std::fs::write(root.join(name), text).unwrap();
    }
    let ready_path = directory.path().join("ready.json");
    let config = Config {
        root: root.to_str().unwrap().into(),
        ready: ready_path.to_str().unwrap().into(),
        entry: "index.html".into(),
    };
    let script = directory.path().join("startup");
    std::fs::write(&script, config.script().unwrap()).unwrap();
    let mut child = Server(
        Command::new(env!("CARGO_BIN_EXE_html-preview"))
            .arg(script)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    let ready: Ready = loop {
        if let Ok(bytes) = std::fs::read(&ready_path) {
            if let Ok(ready) = serde_json::from_slice(&bytes) {
                break ready;
            }
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "server exited before readiness"
        );
        assert!(Instant::now() < deadline, "server readiness timed out");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(ready.protocol, PROTOCOL);
    assert_eq!(ready.root, config.root);
    let port = ready
        .url
        .strip_prefix("http://127.0.0.1:")
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .parse::<u16>()
        .unwrap();
    for (name, text, mime) in [
        ("index.html", HTML, "text/html"),
        ("main.js", JS, "text/javascript"),
        ("style.css", CSS, "text/css"),
    ] {
        let result = request(port, "GET", &format!("/{name}"));
        assert!(result.starts_with("HTTP/1.1 200"));
        assert!(
            result
                .to_ascii_lowercase()
                .contains(&format!("content-type: {mime}"))
        );
        assert!(
            result
                .to_ascii_lowercase()
                .contains("cache-control: no-store")
        );
        assert_eq!(result.split_once("\r\n\r\n").unwrap().1, text);
    }
    std::fs::write(
        root.join("style.css.preview-tmp"),
        "body { background: red; }",
    )
    .unwrap();
    std::fs::rename(root.join("style.css.preview-tmp"), root.join("style.css")).unwrap();
    assert!(request(port, "GET", "/style.css").ends_with("body { background: red; }"));
    assert!(request(port, "HEAD", "/index.html").ends_with("\r\n\r\n"));
    assert!(request(port, "GET", "/%2e%2e/ready.json").starts_with("HTTP/1.1 400"));
}
