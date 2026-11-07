//! Bounded native download ownership. HTTP and TLS run in the kernel service.
use alloc::{format, string::String, vec::Vec};
use core::time::Duration;
use trueos::{netfs, time::{self, Instant}};

pub const TIMEOUT_MS: u32 = 90_000;
pub const CATALOG_LIMIT: usize = 1024 * 1024;
pub const ARCHIVE_LIMIT: usize = 128 * 1024 * 1024;

struct Download(u32);
impl Drop for Download {
    fn drop(&mut self) { let _ = netfs::fetch_bytes_discard(self.0); }
}

fn endpoint(url: &str) -> &'static str {
    match url {
        "https://trueos.eu/apps" => "apps_catalog",
        "https://trueos.eu/probes" => "probes_catalog",
        _ => "archive_or_other",
    }
}

fn failure(code: i32) -> String {
    let category = match code {
        -11 => "timed out",
        -10 => "invalid URL",
        -7 => "exceeds size limit",
        -13 => "TLS failed",
        -12 => "HTTP failed",
        _ => "native transport failed",
    };
    format!("Download {category} ({code})")
}

pub async fn bytes(url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let endpoint = endpoint(url);
    let started = Instant::now();
    super::pullbot::report(&format!("download phase=fetch state=started endpoint={endpoint} timeout_ms={TIMEOUT_MS} max_bytes={limit}"));
    let result = async {
        let download = Download(netfs::fetch_bytes_with_options(url.as_bytes(), TIMEOUT_MS, limit).map_err(failure)?);
        loop {
            match netfs::fetch_bytes_result_len(download.0) {
                Ok(length) if length > limit => return Err(failure(-7)),
                Ok(_) => return netfs::fetch_bytes_read_bounded(download.0, limit).map_err(failure),
                Err(-8) if started.elapsed() < Duration::from_millis(u64::from(TIMEOUT_MS)) => {
                    time::sleep(Duration::from_millis(25)).await;
                }
                Err(-8) => return Err(failure(-11)),
                Err(code) => return Err(failure(code)),
            }
        }
    }.await;
    match &result {
        Ok(bytes) => super::pullbot::report(&format!("download phase=fetch state=complete endpoint={endpoint} elapsed_ms={} bytes={}", started.elapsed().as_millis(), bytes.len())),
        Err(error) => super::pullbot::report(&format!("download phase=fetch state=error endpoint={endpoint} elapsed_ms={} error={error}", started.elapsed().as_millis())),
    }
    result
}
