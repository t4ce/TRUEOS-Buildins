//! Showcases the resume feature.
//!
//! Run with
//!
//! ```not_rust
//! cargo run -q --example with-resume
//! ```

extern crate alloc;

use alloc::{boxed::Box, format, string::ToString, vec};
use futures_util::stream::StreamExt;
use reqwest::{
    header::{ACCEPT_RANGES, RANGE},
    Url,
};
use tokio::{fs::File, io::AsyncWriteExt};
use trauma::path::PathBuf;
use trauma::{download::Download, downloader::Downloader};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn core::error::Error>> {
    // Prepare the download.
    let avatar = Url::parse("https://avatars.githubusercontent.com/u/6969134?v=4").unwrap();
    let output = PathBuf::from("output/avatar.jpg");
    tokio::fs::create_dir_all(output.parent().unwrap()).await?;

    // Make sure the server accepts range requests.
    let res = reqwest::Client::new()
        .head(avatar.to_string())
        .send()
        .await?;
    let headers = res.headers();
    let resumable = match headers.get(ACCEPT_RANGES) {
        None => false,
        Some(x) if x == "none" => false,
        Some(_) => true,
    };
    println!("Is the file resumable: {resumable}");

    // We must ensure that the download is resumable to prove our point.
    assert!(resumable);

    // Request a deterministic partial amount of data to simulate a previously failed download.
    let partial_length = 128_u64;
    let res = reqwest::Client::new()
        .get(avatar.to_string())
        .header(RANGE, format!("bytes=0-{}", partial_length - 1))
        .send()
        .await?;

    // Retrieve the bits.
    let mut stream = res.bytes_stream();
    let mut file = File::create(&output).await?;
    while let Some(item) = stream.next().await {
        file.write_all_buf(&mut item?).await?;
    }
    println!("Retrieved {partial_length} bytes.");

    // Download the rest of the bits with the [`Downloader`].
    let dl = Download::builder()
        .url(avatar)?
        .filename_override(output.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path terminator")
        })?)
        .build();
    let downloads = vec![dl];

    let downloader = Downloader::builder()
        .directory(output.parent().unwrap().to_path_buf())
        .build();
    downloader.download(&downloads).await;

    Ok(())
}
