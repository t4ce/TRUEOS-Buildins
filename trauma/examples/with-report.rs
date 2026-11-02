//! Download files and show a plain-text report.
//!
//! Run with:
//!
//! ```not_rust
//! cargo run -q --example with-report
//! ```

extern crate alloc;

use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
};
use trauma::{
    download::{Download, Status, Summary},
    downloader::Downloader,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn core::error::Error>> {
    // Download the file(s).
    let reqwest_rs = "https://github.com/seanmonstar/reqwest/archive/refs/tags/v0.11.9.zip";
    let fake = format!("{}.fake", &reqwest_rs);
    let downloads = vec![
        Download::builder().url(reqwest_rs)?.build(),
        Download::builder().url(reqwest_rs)?.build(),
        Download::builder().url(fake)?.build(),
    ];
    let downloader = Downloader::builder().directory("output").build();
    let summaries = downloader.download(&downloads).await;

    // Display results.
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
