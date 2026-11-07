//! Represents the download controller.

use crate::path::PathBuf;
use crate::transport::{self, Client as ClientWithMiddleware};
use crate::transport::{
    header::{HeaderMap, ACCEPT_ENCODING, IF_RANGE, RANGE},
    StatusCode,
};
use crate::{
    body::save_body,
    checkpoint::{self, strong_etag, Checkpoint},
    download::{Download, Status, Summary},
    transfer::{self, TransferPlan},
    ResponseExt,
};
use crate::{body::Sink, storage};
#[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
use alloc::sync::Arc;
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use bon::Builder;
use futures_util::stream::{self, StreamExt};
#[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
#[cfg(not(target_os = "trueos"))]
use reqwest_middleware::ClientBuilder;
#[cfg(not(target_os = "trueos"))]
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
#[cfg(not(target_os = "trueos"))]
use tracing::debug;

const DEFAULT_RETRIES: u32 = 3;
const DEFAULT_CONCURRENT_DOWNLOADS: usize = 32;

/// Represents the download controller.
///
/// A downloader can be created via its builder:
///
/// ```rust
/// # fn main()  {
/// use trauma::downloader::Downloader;
///
/// let d = Downloader::builder().build();
/// # }
/// ```
#[derive(Debug, Clone, Builder)]
pub struct Downloader {
    /// Directory where to store the downloaded files.
    #[builder(default = crate::path::default_directory(), into)]
    directory: PathBuf,
    /// Request-level retry budget (body-stream failures are returned to the caller).
    #[builder(default = DEFAULT_RETRIES)]
    retries: u32,
    /// Number of maximum concurrent downloads.
    #[builder(default = DEFAULT_CONCURRENT_DOWNLOADS)]
    concurrent_downloads: usize,
    /// Downloader style options (requires `terminal-progress`).
    #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
    #[builder(default)]
    style_options: StyleOptions,
    /// Resume the download if necessary and possible.
    #[builder(default = true)]
    resumable: bool,
    /// Custom HTTP headers.
    headers: Option<HeaderMap>,
}

impl Default for Downloader {
    fn default() -> Self {
        Downloader::builder().build()
    }
}

impl Downloader {
    const ALREADY_DOWNLOADED: &str = "the file was already fully downloaded";

    /// Starts the downloads. Interrupted bodies leave a checkpointed prefix when
    /// a strong ETag is available; the next call can validate and resume it.
    pub async fn download(&self, downloads: &[Download]) -> Vec<Summary> {
        let prepared = self
            .prepare()
            .expect("the download transport to initialize");
        self.download_inner(downloads, &prepared.client).await
    }

    #[cfg(any(target_os = "trueos", target_os = "zkvm", test))]
    pub(crate) async fn save_received(&self, download: &Download, bytes: &[u8]) -> Status {
        let Some(name) = download.filename_override() else {
            return Status::Fail("received download has no destination".into());
        };
        let output = self.directory.join(name);
        let result = async {
            storage::create_dir_all(output.parent().unwrap_or(&output))
                .await
                .map_err(|e| e.to_string())?;
            checkpoint::invalidate(&output)
                .await
                .map_err(|e| e.to_string())?;
            let mut file = storage::open(&output, 0).await?;
            file.write_all(bytes).await?;
            file.flush().await?;
            #[cfg(any(target_os = "trueos", target_os = "zkvm"))]
            crate::association::completed(&output).await;
            Ok::<(), String>(())
        }
        .await;
        match result {
            Ok(()) => Status::Success,
            Err(error) => Status::Fail(error),
        }
    }

    /// Starts the downloads with proxy.
    #[cfg(not(target_os = "trueos"))]
    pub async fn download_with_proxy(
        &self,
        downloads: &[Download],
        proxy: reqwest::Proxy,
    ) -> Vec<Summary> {
        let prepared = self
            .prepare_with_proxy(Some(proxy))
            .expect("the HTTP client to initialize");
        self.download_inner(downloads, &prepared.client).await
    }

    /// Prepare one shared connection pool and retry middleware for incremental
    /// single downloads. Each call to `download_one` is headless; callers must
    /// bound concurrency and ensure one writer per destination.
    pub fn prepare(&self) -> Result<PreparedDownloader, transport::InitError> {
        #[cfg(not(target_os = "trueos"))]
        {
            self.prepare_with_proxy(None)
        }
        #[cfg(target_os = "trueos")]
        {
            Ok(PreparedDownloader {
                downloader: self.clone(),
                client: ClientWithMiddleware::new(
                    self.headers.clone().unwrap_or_default(),
                    self.retries,
                ),
            })
        }
    }

    #[cfg(not(target_os = "trueos"))]
    fn prepare_with_proxy(
        &self,
        proxy: Option<reqwest::Proxy>,
    ) -> Result<PreparedDownloader, reqwest::Error> {
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(self.retries);
        // Disable automatic decompression even if another dependency unifies
        // Reqwest compression features. Saved offsets always refer to identity bytes.
        let mut builder = reqwest::Client::builder()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd();
        if let Some(proxy) = proxy {
            builder = builder.proxy(proxy);
        }
        let mut headers = self.headers.clone().unwrap_or_default();
        headers.insert(
            ACCEPT_ENCODING,
            reqwest::header::HeaderValue::from_static("identity"),
        );
        let client = ClientBuilder::new(builder.default_headers(headers).build()?)
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();

        Ok(PreparedDownloader {
            downloader: self.clone(),
            client,
        })
    }

    async fn download_inner(
        &self,
        downloads: &[Download],
        client: &ClientWithMiddleware,
    ) -> Vec<Summary> {
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
        let multi = Arc::new(if self.style_options.is_hidden() {
            MultiProgress::with_draw_target(ProgressDrawTarget::hidden())
        } else {
            MultiProgress::new()
        });
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
        let main = Arc::new(
            multi.add(
                self.style_options
                    .main
                    .clone()
                    .to_progress_bar(downloads.len() as u64),
            ),
        );
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
        main.tick();
        let summaries = stream::iter(downloads)
            .map(|download| {
                self.fetch(
                    client,
                    download,
                    #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
                    multi.clone(),
                    #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
                    main.clone(),
                )
            })
            .buffer_unordered(self.concurrent_downloads.max(1))
            .collect::<Vec<_>>()
            .await;
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
        if self.style_options.main.clear {
            main.finish_and_clear();
        } else {
            main.finish();
        }
        summaries
    }

    async fn fetch(
        &self,
        client: &ClientWithMiddleware,
        download: &Download,
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))] multi: Arc<
            MultiProgress,
        >,
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))] main: Arc<
            ProgressBar,
        >,
    ) -> Summary {
        let filename = download
            .filename_override()
            .cloned()
            .or_else(|| download.filename_from_url().ok());
        let mut state = FetchState {
            output: filename.map(|name| self.directory.join(name)),
            statuscode: StatusCode::PROCESSING,
            size: 0,
            resumable: false,
        };
        #[cfg(target_os = "trueos")]
        if let Some(output) = &state.output {
            state.size = storage::file_len(output).await.ok().flatten().unwrap_or(0);
        }
        let status = match self
            .transfer(
                client,
                download,
                &mut state,
                #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
                multi,
            )
            .await
        {
            Ok(status) => status,
            Err(error) => Status::Fail(error),
        };
        #[cfg(any(target_os = "trueos", target_os = "zkvm"))]
        if matches!(status, Status::Success | Status::Skipped(_)) {
            if let Some(output) = &state.output {
                crate::association::completed(output).await;
            }
        }
        // One terminal point reconciles summary lengths for every early return.
        #[cfg(not(target_os = "trueos"))]
        if let Some(output) = &state.output {
            if let Ok(metadata) = tokio::fs::metadata(output).await {
                state.size = metadata.len();
            }
        }
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
        main.inc(1);
        Summary::builder()
            .download(download.clone())
            .statuscode(state.statuscode)
            .size(state.size)
            .resumable(state.resumable)
            .status(status)
            .build()
    }

    async fn transfer(
        &self,
        client: &ClientWithMiddleware,
        download: &Download,
        state: &mut FetchState,
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))] multi: Arc<
            MultiProgress,
        >,
    ) -> Result<Status, String> {
        if let Some(headers) = &self.headers {
            if headers.contains_key(RANGE) || headers.contains_key(IF_RANGE) {
                return Err("Range and If-Range are managed internally; custom values conflict with safe resume".to_string());
            }
            if headers
                .get_all(ACCEPT_ENCODING)
                .iter()
                .any(|v| !v.to_str().is_ok_and(|v| v.eq_ignore_ascii_case("identity")))
            {
                return Err("custom Accept-Encoding must be identity".to_string());
            }
        }
        let head = download.head(client).await.map_err(|e| e.to_string())?;
        state.statuscode = head.status();
        let head = match head.status() {
            StatusCode::OK => Some(head),
            StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED => None,
            status => return Err(format!("unexpected HEAD status {status}")),
        };
        let mut saved = None;
        let mut size = 0;
        if let Some(head) = &head {
            state.output = Some(
                self.directory
                    .join(download.infer_filename(head).ok_or_else(filename_error)?),
            );
            let output = state.output.as_ref().expect("resolved filename");
            size = disk_len(output).await.map_err(|e| e.to_string())?;
            state.size = size;
            if self.resumable {
                saved = Checkpoint::load(output).await.map_err(|e| e.to_string())?;
                if let Some(checkpoint) = &saved {
                    if checkpoint.complete
                        && checkpoint.total == Some(size)
                        && storage::file_len(output)
                            .await
                            .is_ok_and(|length| length.is_some())
                        && head.content_length_header() == Some(size)
                        && transfer::identity_encoding(head.headers()).is_ok()
                        && !head
                            .headers()
                            .contains_key(transport::header::CONTENT_RANGE)
                        && checkpoint.matches(
                            &download.url(),
                            head.url(),
                            strong_etag(head.headers()).as_deref(),
                        )
                    {
                        state.resumable = head.accept_ranges();
                        return Ok(Status::Skipped(Self::ALREADY_DOWNLOADED.to_string()));
                    }
                }
            }
        }
        let resume = saved.as_ref().filter(|checkpoint| {
            head.as_ref()
                .is_some_and(|head| transfer::can_resume(head, checkpoint, size, &download.url()))
        });
        let mut headers = HeaderMap::new();
        if let Some(checkpoint) = resume {
            headers.insert(
                RANGE,
                transport::header::HeaderValue::from_str(&format!("bytes={size}-"))
                    .map_err(|_| "invalid resume range")?,
            );
            headers.insert(
                IF_RANGE,
                transport::header::HeaderValue::from_str(&checkpoint.etag)
                    .map_err(|_| "invalid resume validator")?,
            );
        }
        let response = transport::send(client, transport::Method::GET, download.url(), headers)
            .await
            .map_err(|e| e.to_string())?;
        state.statuscode = response.status();
        let plan = TransferPlan::validate(&response, resume.map(|checkpoint| (checkpoint, size)))?;
        if plan.offset > 0
            && head.as_ref().and_then(ResponseExt::content_length_header) != plan.total
        {
            return Err("partial response disagrees with HEAD representation length".to_string());
        }
        if head.is_none() {
            state.output = Some(
                self.directory.join(
                    download
                        .infer_filename(&response)
                        .ok_or_else(filename_error)?,
                ),
            );
        }
        let output = state.output.as_ref().expect("resolved filename");
        let checkpoint = if self.resumable {
            strong_etag(response.headers())
                .map(|etag| Checkpoint::new(&download.url(), response.url(), etag, plan.total))
        } else {
            None
        };
        state.resumable = checkpoint.is_some()
            && (plan.offset > 0
                || response.accept_ranges()
                || head.as_ref().is_some_and(ResponseExt::accept_ranges));
        let mut progress = TransferProgress::new(
            #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
            &multi,
            #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
            &self.style_options.child,
            plan.total,
            plan.offset,
            #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
            output
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        );
        storage::create_dir_all(output.parent().unwrap_or(output))
            .await
            .map_err(|e| e.to_string())?;
        // Never leave an old representation checkpoint attached to a truncated file.
        if plan.offset == 0 {
            checkpoint::invalidate(output)
                .await
                .map_err(|e| e.to_string())?;
        }
        #[cfg(not(target_os = "trueos"))]
        debug!("Opening destination {:?} at offset {}", output, plan.offset);
        let mut file = storage::open(output, plan.offset).await?;
        state.size = file.committed_len().await?;
        if let Some(checkpoint) = &checkpoint {
            checkpoint.save(output).await.map_err(|e| e.to_string())?;
        }
        let result = save_body(
            response.bytes_stream(),
            &mut file,
            &plan,
            checkpoint,
            output,
            |position| progress.set_position(position),
        )
        .await;
        state.size = result.size;
        if let Some(error) = result.error {
            return Err(error);
        }
        progress.finish();
        Ok(Status::Success)
    }
}

/// A reusable headless download context with a shared HTTP connection pool.
#[derive(Clone)]
pub struct PreparedDownloader {
    downloader: Downloader,
    client: ClientWithMiddleware,
}
impl PreparedDownloader {
    /// Download one item without terminal output. This preserves checkpoint,
    /// retry and summary behavior of `Downloader::download`.
    pub async fn download_one(&self, download: &Download) -> Summary {
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
        let multi = Arc::new(MultiProgress::with_draw_target(ProgressDrawTarget::hidden()));
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
        let main = Arc::new(multi.add(ProgressBar::hidden()));
        self.downloader
            .fetch(
                &self.client,
                download,
                #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
                multi,
                #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
                main,
            )
            .await
    }
}

struct FetchState {
    output: Option<PathBuf>,
    statuscode: StatusCode,
    size: u64,
    resumable: bool,
}

fn filename_error() -> String {
    "Cannot extract the filename. Verify the URL and/or provide an override.".to_string()
}

async fn disk_len(path: &crate::path::Path) -> Result<u64, String> {
    Ok(storage::file_len(path).await?.unwrap_or(0))
}

#[cfg(all(test, feature = "terminal-progress"))]
std::thread_local! {
    static TEST_CHILDREN: core::cell::RefCell<Vec<ProgressBar>> = const { core::cell::RefCell::new(Vec::new()) };
}

// A child is finalized even when opening, streaming, flushing or checkpoint
// publication returns early. The lean build carries no renderer or shared bars.
struct TransferProgress {
    #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
    bar: ProgressBar,
    #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
    clear: bool,
    #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
    multi: MultiProgress,
}
impl TransferProgress {
    fn new(
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))] multi: &MultiProgress,
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
        options: &ProgressBarOpts,
        total: Option<u64>,
        offset: u64,
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))] filename: String,
    ) -> Self {
        #[cfg(not(feature = "terminal-progress"))]
        let _ = (total, offset);
        Self {
            #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
            bar: {
                let bar = options.clone().to_progress_bar(total.unwrap_or(0));
                if total.is_none() {
                    bar.unset_length();
                }
                let bar = multi.add(bar.with_position(offset).with_message(filename));
                #[cfg(test)]
                TEST_CHILDREN.with(|children| children.borrow_mut().push(bar.clone()));
                bar
            },
            #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
            clear: options.clear,
            #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
            multi: multi.clone(),
        }
    }
    fn set_position(&self, position: u64) {
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
        self.bar.set_position(position);
        #[cfg(not(feature = "terminal-progress"))]
        let _ = position;
    }
    fn finish(&mut self) {
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
        if self.clear {
            self.bar.finish_and_clear();
        } else {
            self.bar.finish();
        }
    }
}
impl Drop for TransferProgress {
    fn drop(&mut self) {
        #[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
        if !self.bar.is_finished() {
            // Abandon preserves the committed position; finish_and_clear would
            // first advance it to the expected total, even on a failed transfer.
            self.bar.abandon();
            if self.clear {
                self.multi.remove(&self.bar);
            }
        }
    }
}

/// Define the [`Downloader`] options.
///
/// By default, the main progress bar will stay on the screen upon completion,
/// but the child ones will be cleared once complete.
#[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
#[derive(Debug, Clone, Builder)]
pub struct StyleOptions {
    /// Style options for the main progress bar.
    main: ProgressBarOpts,
    /// Style options for the child progress bar(s).
    child: ProgressBarOpts,
}

#[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
impl Default for StyleOptions {
    fn default() -> Self {
        Self::builder()
            .main(
                ProgressBarOpts::builder()
                    .template(ProgressBarOpts::TEMPLATE_BAR_WITH_POSITION)
                    .progress_chars(ProgressBarOpts::CHARS_FINE)
                    .build(),
            )
            .child(ProgressBarOpts::with_pip_style())
            .build()
    }
}

#[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
impl StyleOptions {
    /// Convenience function to create a style hidding the progress bars.
    pub fn hidden() -> Self {
        Self::builder()
            .main(ProgressBarOpts::hidden())
            .child(ProgressBarOpts::hidden())
            .build()
    }

    /// Set the options for the main progress bar.
    pub fn set_main(&mut self, main: ProgressBarOpts) {
        self.main = main;
    }

    /// Set the options for the child progress bar.
    pub fn set_child(&mut self, child: ProgressBarOpts) {
        self.child = child;
    }

    /// Check whether both progress bars are hidden.
    pub fn is_hidden(&self) -> bool {
        self.main.hidden && self.child.hidden
    }
}

/// Define the options for a progress bar.
#[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
#[derive(Debug, Clone, Builder)]
#[builder(on(String, into))]
pub struct ProgressBarOpts {
    /// Progress bar template string.
    template: Option<String>,
    /// Progression characters set.
    ///
    /// There must be at least 3 characters for the following states:
    /// "filled", "current", and "to do".
    progress_chars: Option<String>,
    /// Hide the progress bar.
    #[builder(default,  with = || true)]
    hidden: bool,
    /// Clear the progress bar once completed.
    #[builder(default, with = || true)]
    clear: bool,
}

#[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
impl Default for ProgressBarOpts {
    fn default() -> Self {
        Self::builder().build()
    }
}

#[cfg(all(feature = "terminal-progress", not(target_os = "trueos")))]
impl ProgressBarOpts {
    /// Template representing the bar and its position.
    ///
    ///`█████████████████████ 11/12 (99%) eta 00:00:02 archive.zip`
    pub const TEMPLATE_BAR_WITH_POSITION: &'static str =
        "{bar:20.blue} {pos:>4}/{len:4} ({percent}%) eta {eta_precise:.blue} {msg}";
    /// Template which looks like the Python package installer pip.
    ///
    /// `━━━━━╾──────────────    3.78 MiB/13.43 MiB    191.39 KiB/s eta 52s  archive.zip`
    pub const TEMPLATE_PIP: &'static str =
        "{bar:20.green/black} {bytes:>11.green}/{total_bytes:<11.green} {bytes_per_sec:>13.red} eta {eta:4.blue} {msg}";
    /// Use increasing quarter blocks as progress characters: `"█▛▌▖  "`.
    pub const CHARS_BLOCKY: &'static str = "█▛▌▖  ";
    /// Use fade-in blocks as progress characters: `"█▓▒░  "`.
    pub const CHARS_FADE_IN: &'static str = "█▓▒░  ";
    /// Use fine blocks as progress characters: `"█▉▊▋▌▍▎▏  "`.
    pub const CHARS_FINE: &'static str = "█▉▊▋▌▍▎▏  ";
    /// Use a line as progress characters: `"━╾─"`.
    pub const CHARS_LINE: &'static str = "━╾╴─";
    /// Use rough blocks as progress characters: `"█  "`.
    pub const CHARS_ROUGH: &'static str = "█  ";
    /// Use increasing height blocks as progress characters: `"█▇▆▅▄▃▂▁  "`.
    pub const CHARS_VERTICAL: &'static str = "█▇▆▅▄▃▂▁  ";

    /// Create a [`ProgressStyle`] based on the provided options.
    pub fn to_progress_style(self) -> ProgressStyle {
        let mut style = ProgressStyle::default_bar();
        if let Some(template) = self.template {
            style = style.template(&template).unwrap();
        }
        if let Some(progress_chars) = self.progress_chars {
            style = style.progress_chars(&progress_chars);
        }
        style
    }

    /// Create a [`ProgressBar`] based on the provided options.
    pub fn to_progress_bar(self, len: u64) -> ProgressBar {
        // Return a hidden Progress bar if we hid it.
        if self.hidden {
            return ProgressBar::hidden();
        }

        // Otherwise returns a ProgressBar with the style.
        let style = self.to_progress_style();
        ProgressBar::new(len).with_style(style)
    }

    /// Create a new [`ProgressBarOpts`] which looks like Python pip.
    pub fn with_pip_style() -> Self {
        Self::builder()
            .template(ProgressBarOpts::TEMPLATE_PIP)
            .progress_chars(ProgressBarOpts::CHARS_LINE)
            .clear()
            .build()
    }

    /// Create a new [`ProgressBarOpts`] which hides the progress bars.
    pub fn hidden() -> Self {
        Self::builder().hidden().build()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_builder_defaults() {
        let d = Downloader::builder().build();
        assert_eq!(d.retries, DEFAULT_RETRIES);
        assert_eq!(d.concurrent_downloads, DEFAULT_CONCURRENT_DOWNLOADS);
        assert!(d.resumable);
    }

    #[test]
    fn test_builder_resumable_toggle() {
        let d = Downloader::builder().resumable(false).build();
        assert!(!d.resumable);
    }
}

#[cfg(all(test, feature = "terminal-progress"))]
mod progress_tests {
    use super::*;
    use crate::test_support::{Directory, Reply, Server};
    #[test]
    fn progress_uses_full_length_and_failure_drop_preserves_written_position() {
        TEST_CHILDREN.with(|children| children.borrow_mut().clear());
        let multi = MultiProgress::with_draw_target(ProgressDrawTarget::hidden());
        let options = ProgressBarOpts::default();
        let progress =
            TransferProgress::new(&multi, &options, Some(1000), 500, "resume".to_string());
        let bar = progress.bar.clone();
        assert_eq!(bar.length(), Some(1000));
        assert_eq!(bar.position(), 500);
        progress.set_position(600);
        drop(progress);
        assert!(bar.is_finished());
        assert_eq!(bar.position(), 600);
        let clear_options = ProgressBarOpts::builder().clear().build();
        let cleared = TransferProgress::new(
            &multi,
            &clear_options,
            Some(1000),
            500,
            "cleared failure".to_string(),
        );
        let cleared_bar = cleared.bar.clone();
        drop(cleared);
        assert!(cleared_bar.is_finished());
        assert_eq!(cleared_bar.position(), 500);
        let mut unknown = TransferProgress::new(&multi, &options, None, 0, "unknown".to_string());
        assert_eq!(unknown.bar.length(), None);
        unknown.set_position(12);
        unknown.finish();
        assert!(unknown.bar.is_finished());
        assert_eq!(unknown.bar.position(), 12);
        TEST_CHILDREN.with(|children| children.borrow_mut().clear());
    }

    #[test]
    fn mixed_http_batch_counts_each_summary_and_finalizes_every_created_child() {
        let directory = Directory::new();
        let server = Server::new(alloc::vec![
            Reply::metadata("HEAD", 4, "\"v1\""),
            Reply::metadata("GET", 4, "\"v1\"").body(b"done"),
            Reply::metadata("HEAD", 4, "\"v1\""),
            Reply::metadata("GET", 4, "\"v1\"").body(b"good"),
            Reply::metadata("HEAD", 4, "\"v1\""),
            Reply::new("HEAD", 403).header("Content-Length", 0),
            Reply::metadata("HEAD", 4, "\"v1\""),
            Reply::new("GET", 404).header("Content-Length", 0),
            Reply::metadata("HEAD", 1000, "\"v1\""),
            Reply::partial(1000, alloc::vec![b'a'; 500]),
        ]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let downloader = Downloader::builder()
                .directory(directory.path())
                .retries(0)
                .concurrent_downloads(1)
                .style_options(
                    StyleOptions::builder()
                        .main(ProgressBarOpts::default())
                        .child(ProgressBarOpts::default())
                        .build(),
                )
                .build();
            let complete = Download::builder().url(&server.url).unwrap().build();
            assert_eq!(
                downloader.download(&[complete.clone()]).await[0].status(),
                &Status::Success
            );
            TEST_CHILDREN.with(|children| children.borrow_mut().clear());
            let multi = Arc::new(MultiProgress::with_draw_target(ProgressDrawTarget::hidden()));
            let main = Arc::new(multi.add(ProgressBar::new(5)));
            let client = ClientBuilder::new(reqwest::Client::new()).build();
            let downloads: Vec<_> = [
                "success.bin",
                "fixture.bin",
                "head-failure.bin",
                "get-failure.bin",
                "interrupted.bin",
            ]
            .into_iter()
            .map(|name| {
                Download::builder()
                    .url(server.url.replace("fixture.bin", name))
                    .unwrap()
                    .build()
            })
            .collect();
            let summaries = stream::iter(&downloads)
                .map(|download| downloader.fetch(&client, download, multi.clone(), main.clone()))
                .buffer_unordered(1)
                .collect::<Vec<_>>()
                .await;
            assert_eq!(summaries[0].status(), &Status::Success);
            assert!(matches!(summaries[1].status(), Status::Skipped(_)));
            assert!(summaries[2..]
                .iter()
                .all(|s| matches!(s.status(), Status::Fail(_))));
            assert_eq!(
                summaries.iter().map(Summary::size).collect::<Vec<_>>(),
                [4, 4, 0, 0, 500]
            );
            assert_eq!(main.position(), 5);
            TEST_CHILDREN.with(|children| {
                let children = children.borrow();
                assert_eq!(children.len(), 2);
                assert!(children.iter().all(ProgressBar::is_finished));
                assert_eq!(children[1].length(), Some(1000));
                assert_eq!(children[1].position(), 500);
            });
            TEST_CHILDREN.with(|children| children.borrow_mut().clear());
        });
        server.finish();
    }
}
