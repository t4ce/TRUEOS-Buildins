//! Private, versioned representation checkpoints. One writer per destination.
use crate::{
    checkpoint_rename,
    path::{Path, PathBuf},
};
use alloc::{
    format,
    string::{String, ToString},
};
use core::sync::atomic::{AtomicU64, Ordering};
use reqwest::{
    header::{HeaderMap, ETAG},
    Url,
};
use sha2::{Digest, Sha256};
use std::io;
use tokio::io::AsyncWriteExt;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
const MAX_CHECKPOINT: u64 = 16 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Checkpoint {
    pub requested: String,
    pub resolved: String,
    pub etag: String,
    pub total: Option<u64>,
    pub complete: bool,
}

pub(crate) fn fingerprint(url: &Url) -> String {
    // Never persist credentials, query parameters, or redirects as plaintext.
    format!("{:x}", Sha256::digest(url.as_str().as_bytes()))
}

pub(crate) fn strong_etag(headers: &HeaderMap) -> Option<String> {
    if headers.get_all(ETAG).iter().count() != 1 {
        return None;
    }
    let value = headers.get(ETAG)?.to_str().ok()?;
    if valid_etag(value) {
        Some(value.to_string())
    } else {
        None
    }
}

fn valid_etag(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && bytes[0] == b'"'
        && bytes[bytes.len() - 1] == b'"'
        && bytes[1..bytes.len() - 1]
            .iter()
            .all(|b| *b == 0x21 || (0x23..=0x7e).contains(b))
        && value.len() < 8192
}

pub(crate) fn path(output: &Path) -> PathBuf {
    let mut name = output.as_os_str().to_os_string();
    name.push(".trauma-resume");
    PathBuf::from(name)
}

impl Checkpoint {
    pub fn new(requested: &Url, resolved: &Url, etag: String, total: Option<u64>) -> Self {
        Self {
            requested: fingerprint(requested),
            resolved: fingerprint(resolved),
            etag,
            total,
            complete: false,
        }
    }
    pub fn matches(&self, requested: &Url, resolved: &Url, etag: Option<&str>) -> bool {
        self.requested == fingerprint(requested)
            && self.resolved == fingerprint(resolved)
            && etag == Some(self.etag.as_str())
    }
    fn encode(&self) -> String {
        format!(
            "trauma-resume-v1\n{}\n{}\n{}\n{}\n{}\n",
            self.requested,
            self.resolved,
            self.etag,
            self.total
                .map_or_else(|| "-".to_string(), |n| n.to_string()),
            if self.complete { "complete" } else { "partial" }
        )
    }
    fn parse(value: &str) -> Option<Self> {
        let mut lines = value.lines();
        if lines.next()? != "trauma-resume-v1" {
            return None;
        }
        let requested = lines.next()?;
        let resolved = lines.next()?;
        for hash in [requested, resolved] {
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return None;
            }
        }
        let etag = lines.next()?;
        if !valid_etag(etag) {
            return None;
        }
        let total = match lines.next()? {
            "-" => None,
            n => Some(n.parse().ok()?),
        };
        let complete = match lines.next()? {
            "partial" => false,
            "complete" => true,
            _ => return None,
        };
        if lines.next().is_some() || (complete && total.is_none()) {
            return None;
        }
        Some(Self {
            requested: requested.to_string(),
            resolved: resolved.to_string(),
            etag: etag.to_string(),
            total,
            complete,
        })
    }
    pub async fn load(output: &Path) -> io::Result<Option<Self>> {
        let path = path(output);
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if metadata.len() > MAX_CHECKPOINT {
            return Ok(None);
        }
        let data = tokio::fs::read(path).await?;
        Ok(core::str::from_utf8(&data).ok().and_then(Self::parse))
    }
    pub async fn save(&self, output: &Path) -> io::Result<()> {
        let destination = path(output);
        // create_new avoids trampling another file, even after a process crash.
        let (temporary, mut file) = loop {
            let mut name = destination.as_os_str().to_os_string();
            name.push(format!(
                ".tmp-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            let temporary = PathBuf::from(name);
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .await
            {
                Ok(file) => break (temporary, file),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        };
        let result = async {
            file.write_all(self.encode().as_bytes()).await?;
            file.flush().await?;
            drop(file);
            checkpoint_rename::replace(&temporary, &destination).await
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&temporary).await;
        }
        result
    }
}

pub(crate) async fn invalidate(output: &Path) -> io::Result<()> {
    match tokio::fs::remove_file(path(output)).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
