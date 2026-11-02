//! Download example where a file cannot be resumed.
//!
//! Run with
//!
//! ```not_rust
//! cargo run -q --example with-not-resumable
//! ```

extern crate alloc;

use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec,
};
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
};
use trauma::path::PathBuf;
use trauma::{
    download::{Download, Status, Summary},
    downloader::Downloader,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn core::error::Error>> {
    // Prepare the download.
    // This URL is known to return a Content-Length: 0 to HEAD requests.
    let url = "https://stateparks.oregon.gov/index.cfm?do=main.loadFile&load=_siteFiles%2Fpublications%2F%2FSpringValleyGeoPDF093621.pdf";
    let output_dir = PathBuf::from("output");
    let filename = "SpringValleyGeoPDF093621.pdf";
    let output = output_dir.join(filename);

    // Clean up.
    let _ = fs::remove_file(&output).await;
    fs::create_dir_all(&output_dir).await?;

    // Create a corrupted file for testing that it does get re-downloaded.
    let mut file = File::create(&output).await?;
    file.write_all(b"hello").await?;
    file.flush().await?;
    println!(
        "Created a corrupted file: {} bytes.",
        file.metadata().await?.len()
    );

    // Setup the download item for Trauma.
    let downloads = vec![Download::builder()
        .url(url)?
        .filename_override(filename)
        .build()];
    let downloader = Downloader::builder().directory(output_dir).build();

    // First attempt, redownload the file and display the summary.
    print!("Downloading file... ");
    let summaries = downloader.download(&downloads).await;
    display_summary(&summaries);

    // Second attempt, the file is already fully downloaded, so it should be skipped.
    println!("Downloading file again... ");
    let summaries = downloader.download(&downloads).await;
    display_summary(&summaries);

    Ok(())
}

fn display_summary(summaries: &[Summary]) {
    println!("{:<40} {:>12} {:<10} Error", "File", "Size", "Status");
    for s in summaries {
        let mut error = String::new();
        let status = match s.status() {
            Status::Success => String::from("✅"),
            Status::Fail(s) => {
                error = s.to_string();
                if error.chars().count() > 50 {
                    error = error.chars().take(50).collect();
                    error.push_str("...");
                }
                String::from("❌")
            }
            Status::NotStarted => String::from("🔜"),
            Status::Skipped(s) => {
                error = s.to_string();
                String::from("⏭️")
            }
        };
        let filename = s
            .download()
            .filename_override()
            .cloned()
            .or_else(|| s.download().filename_from_url().ok())
            .unwrap_or_else(|| "<unknown>".to_string());
        println!("{filename:<40} {:>12} {status:<10} {error}", s.size());
    }
}
