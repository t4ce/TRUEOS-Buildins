//! Refresh only pages marked by IDEdit's local development Blueprint.
#[cfg(test)]
use alloc::format;
use alloc::{string::String};
use crate::execution::{Duration, Instant};
use trueos::netfs;
use trueos_ide::html_preview::{REVISION_MARKER, REVISION_PATH};

pub(crate) struct Watch {
    revision_url: String,
    revision: u64,
    next: Instant,
    pending: Option<(u32, Instant)>,
}
impl Watch {
    pub(crate) fn from_page(page_url: &str, html: &[u8]) -> Option<Self> {
        let url = url::Url::parse(page_url).ok()?;
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || url.port().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        let first_line = html.split(|byte| *byte == b'\n').next()?;
        if first_line.len() > 100 {
            return None;
        }
        let marker = core::str::from_utf8(first_line).ok()?;
        let revision = marker
            .strip_prefix(REVISION_MARKER)?
            .strip_suffix(" -->")?
            .parse()
            .ok()?;
        Some(Self {
            revision_url: url.join(REVISION_PATH).ok()?.into(),
            revision,
            next: Instant::now() + Duration::from_millis(750),
            pending: None,
        })
    }
    /// The normal event loop polls this without blocking rendering or input.
    pub(crate) fn poll(&mut self) -> bool {
        let now = Instant::now();
        if let Some((operation, started)) = self.pending {
            let bytes = match netfs::fetch_bytes_result_len(operation) {
                Err(-8) if now.duration_since(started) < Duration::from_secs(3) => return false,
                Ok(len) if len <= 32 => netfs::fetch_bytes_read(operation).ok(),
                _ => None,
            };
            let _ = netfs::fetch_bytes_discard(operation);
            self.pending = None;
            self.next = now + Duration::from_millis(750);
            if let Some(revision) =
                bytes.and_then(|b| core::str::from_utf8(&b).ok()?.parse::<u64>().ok())
            {
                return revision != self.revision;
            }
        } else if now >= self.next {
            self.next = now + Duration::from_millis(750);
            if let Ok(operation) = netfs::fetch_bytes_happy_eyeballs(self.revision_url.as_bytes()) {
                self.pending = Some((operation, now));
            }
        }
        false
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        if let Some((operation, _)) = self.pending.take() {
            let _ = netfs::fetch_bytes_discard(operation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_marked_local_development_pages_enable_refresh() {
        let html = format!("{REVISION_MARKER}4 -->\n<!doctype html><title>こんにちは</title>");
        let watch =
            Watch::from_page("http://127.0.0.1:1234/nested/page.html", html.as_bytes()).unwrap();
        assert_eq!(watch.revision, 4);
        assert_eq!(
            watch.revision_url,
            "http://127.0.0.1:1234/__idedit/revision"
        );
        assert!(Watch::from_page("https://example.org/", html.as_bytes()).is_none());
        assert!(Watch::from_page("http://127.0.0.1:1234/", b"<!doctype html>").is_none());
    }
    #[test]
    fn saved_revision_refreshes_once_and_navigation_discards_pending_work() {
        let revision_url = "http://127.0.0.1:1234/__idedit/revision";
        crate::netfs::install([(revision_url.into(), b"4".to_vec())]);
        let html = format!("{REVISION_MARKER}4 -->\n");
        let mut watch =
            Watch::from_page("http://127.0.0.1:1234/index.html", html.as_bytes()).unwrap();
        watch.next = Instant::now();
        assert!(!watch.poll());

        assert!(!watch.poll());
        crate::netfs::install([(revision_url.into(), b"5".to_vec())]);
        watch.next = Instant::now();
        assert!(!watch.poll());
        assert!(watch.poll());
        watch.next = Instant::now();
        assert!(!watch.poll());
        assert_eq!(crate::netfs::active(), 1);
        drop(watch);
        assert_eq!(crate::netfs::active(), 0);
    }
}
