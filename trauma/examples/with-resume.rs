//! Host-side loopback demonstration of checkpointed resume and verified skip.
//!
//! The fixture interrupts a 1,000-byte response after 500 bytes. The next call
//! resumes with Range + If-Range, and a third call verifies the completed file.
//! No external server or hand-written checkpoint is needed.
//!
//! Run through `python3 tools/check_host.py` for this TRUEOS source copy.
extern crate alloc;

use alloc::{boxed::Box, vec};
use trauma::{
    download::{Download, Status},
    downloader::Downloader,
    path,
};

#[path = "../tests/support/mod.rs"]
mod support;
use support::{Directory, Reply, Server};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn core::error::Error>> {
    let directory = Directory::new();
    let contents = [vec![b'a'; 500], vec![b'b'; 500]].concat();
    let server = Server::new(vec![
        Reply::metadata("HEAD", 1000, "\"v1\""),
        Reply::partial(1000, contents[..500].to_vec()),
        Reply::metadata("HEAD", 1000, "\"v1\""),
        Reply::new("GET", 206)
            .header("ETag", "\"v1\"")
            .header("Content-Range", "bytes 500-999/1000")
            .header("Content-Length", 500)
            .body(contents[500..].to_vec()),
        Reply::metadata("HEAD", 1000, "\"v1\""),
    ]);
    let downloads = vec![Download::builder().url(&server.url)?.build()];
    let downloader = Downloader::builder()
        .directory(directory.path())
        .retries(0)
        .build();

    let first = downloader.download(&downloads).await;
    assert!(matches!(first[0].status(), Status::Fail(_)));
    assert_eq!(first[0].size(), 500);
    println!("Interrupted response: 500 bytes saved with a partial checkpoint.");

    let resumed = downloader.download(&downloads).await;
    assert_eq!(resumed[0].status(), &Status::Success);
    assert_eq!(resumed[0].size(), 1000);
    assert_eq!(directory.bytes(), contents);
    println!("Validated resumed response: all 1,000 bytes saved.");

    let verified = downloader.download(&downloads).await;
    assert!(matches!(verified[0].status(), Status::Skipped(_)));
    assert_eq!(verified[0].size(), 1000);
    let requests = server.finish();
    assert_eq!(requests[3].header("range"), Some("bytes=500-"));
    assert_eq!(requests[3].header("if-range"), Some("\"v1\""));
    println!("Matching remote identity: completed file verified and skipped.");
    Ok(())
}
