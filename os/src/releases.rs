//! GitHub release discovery; the selected asset URL is handed to the host updater.
use alloc::{format, string::String, vec::Vec};
use core::time::Duration;
use trueos::{netfs, time::{self, Instant}};

const API: &str = "https://api.github.com/repos/t4ce/TRUEOS/releases?per_page=3";
const LIMIT: usize = 1024 * 1024;
const TIMEOUT_MS: u32 = 15_000;

pub struct Release {
    pub version: String,
    pub asset: String,
    pub url: String,
}

struct Download(u32);
impl Drop for Download {
    fn drop(&mut self) { let _ = netfs::fetch_bytes_discard(self.0); }
}

pub async fn fetch() -> Result<Vec<Release>, String> {
    let error = |code| format!("GitHub releases unavailable ({code}). Choose Refresh to retry.");
    let download = Download(netfs::fetch_bytes_with_options(API.as_bytes(), TIMEOUT_MS, LIMIT).map_err(error)?);
    let started = Instant::now();
    loop {
        match netfs::fetch_bytes_result_len(download.0) {
            Ok(_) => {
                let bytes = netfs::fetch_bytes_read_bounded(download.0, LIMIT).map_err(error)?;
                return parse(&bytes);
            }
            Err(-8) if started.elapsed() < Duration::from_millis(u64::from(TIMEOUT_MS)) => {
                time::sleep(Duration::from_millis(25)).await;
            }
            Err(code) => return Err(error(code)),
        }
    }
}

pub fn parse(bytes: &[u8]) -> Result<Vec<Release>, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| String::from("GitHub returned invalid release data."))?;
    let entries = value.as_array().ok_or_else(|| String::from("GitHub releases unavailable; retry later."))?;
    let mut releases = Vec::new();
    for entry in entries.iter().take(3) {
        if entry["draft"].as_bool() == Some(true) { continue; }
        let Some(version) = entry["tag_name"].as_str() else { continue; };
        let Some(assets) = entry["assets"].as_array() else { continue; };
        let expected = format!("TrueOS-{}.lz4", version.strip_prefix('v').unwrap_or(version));
        let Some(asset) = assets.iter().find(|asset| asset["name"].as_str() == Some(expected.as_str())) else { continue; };
        let Some(url) = asset["browser_download_url"].as_str() else { continue; };
        if !url.starts_with("https://github.com/t4ce/TRUEOS/releases/download/")
            || !url.ends_with(".lz4") || url.len() > 256
            || url.chars().any(|ch| ch.is_control() || ch.is_whitespace()) { continue; }
        let version = version.chars().filter(|ch| !ch.is_control()).collect();
        releases.push(Release { version, asset: expected, url: String::from(url) });
    }
    if releases.is_empty() { return Err(String::from("No downloadable TRUEOS releases found.")); }
    Ok(releases)
}
