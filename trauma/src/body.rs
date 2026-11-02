//! Byte accounting is based on successful writes, reconciled after flushing.
use alloc::{
    format,
    string::{String, ToString},
};
use core::fmt::Display;
use futures_util::{Stream, StreamExt};
use std::io;
use tokio::io::{AsyncWrite, AsyncWriteExt};

pub(crate) trait Sink: AsyncWrite + Unpin {
    async fn committed_len(&mut self) -> io::Result<u64>;
}
impl Sink for tokio::fs::File {
    async fn committed_len(&mut self) -> io::Result<u64> {
        Ok(self.metadata().await?.len())
    }
}

pub(crate) struct BodyResult {
    pub size: u64,
    pub error: Option<String>,
}

async fn write_body<S, B, E, W>(
    stream: S,
    sink: &mut W,
    offset: u64,
    total: Option<u64>,
    mut progress: impl FnMut(u64),
) -> BodyResult
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: Display,
    W: Sink,
{
    futures_util::pin_mut!(stream);
    let mut size = offset;
    let mut error = None;
    while let Some(item) = stream.next().await {
        let chunk = match item {
            Ok(chunk) => chunk,
            Err(e) => {
                error = Some(e.to_string());
                break;
            }
        };
        let bytes = chunk.as_ref();
        let next = match size.checked_add(bytes.len() as u64) {
            Some(next) if total.is_none_or(|n| next <= n) => next,
            _ => {
                error = Some("body exceeds the expected representation length".to_string());
                break;
            }
        };
        if let Err(e) = sink.write_all(bytes).await {
            error = Some(e.to_string());
            break;
        }
        size = next;
        progress(size);
    }
    // Tokio File writes can be queued. Flush on failures as well as EOF before
    // inspecting metadata or allowing the caller to retry this saved prefix.
    if let Err(e) = sink.flush().await {
        error.get_or_insert_with(|| format!("flush failed: {e}"));
    }
    match sink.committed_len().await {
        Ok(actual) => {
            size = actual;
            progress(size);
        }
        Err(e) => {
            error.get_or_insert_with(|| format!("cannot determine saved length: {e}"));
        }
    }
    if error.is_none() && total.is_some_and(|n| size != n) {
        error = Some("body ended before the expected representation length".to_string());
    }
    BodyResult { size, error }
}

pub(crate) async fn save_body<S, B, E, W>(
    stream: S,
    sink: &mut W,
    plan: &crate::transfer::TransferPlan,
    mut checkpoint: Option<crate::checkpoint::Checkpoint>,
    output: &crate::path::Path,
    progress: impl FnMut(u64),
) -> BodyResult
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: Display,
    W: Sink,
{
    let mut result = write_body(stream, sink, plan.offset, plan.total, progress).await;
    if result.error.is_none() {
        if let Some(checkpoint) = &mut checkpoint {
            checkpoint.total = Some(result.size);
            checkpoint.complete = true;
            if let Err(error) = checkpoint.save(output).await {
                result.error = Some(error.to_string());
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support as support;
    use crate::{checkpoint::Checkpoint, transfer::TransferPlan};
    use core::{
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::AsyncWrite;

    // This sink writes real bytes to a real Tokio File, then fails after a
    // partial write or after flushing. Metadata reconciliation sees the actual
    // file, not a mocked count that mirrors the implementation's arithmetic.
    struct FaultSink {
        file: tokio::fs::File,
        remaining: Option<usize>,
        fail_flush: bool,
    }
    impl AsyncWrite for FaultSink {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            if self.remaining == Some(0) {
                return Poll::Ready(Err(io::Error::other("injected partial write failure")));
            }
            let count = self.remaining.map_or(bytes.len(), |n| n.min(bytes.len()));
            let result = Pin::new(&mut self.file).poll_write(cx, &bytes[..count]);
            if let Poll::Ready(Ok(written)) = &result {
                if let Some(remaining) = &mut self.remaining {
                    *remaining -= written;
                }
            }
            result
        }
        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            match Pin::new(&mut self.file).poll_flush(cx) {
                Poll::Ready(Ok(())) if self.fail_flush => {
                    Poll::Ready(Err(io::Error::other("injected flush failure")))
                }
                result => result,
            }
        }
        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.file).poll_shutdown(cx)
        }
    }
    impl Sink for FaultSink {
        async fn committed_len(&mut self) -> io::Result<u64> {
            Ok(self.file.metadata().await?.len())
        }
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    #[test]
    fn partial_write_and_flush_failures_report_actual_saved_bytes_without_completion() {
        for flush_failure in [false, true] {
            let directory = support::Directory::new();
            std::fs::write(directory.file(), b"old").unwrap();
            runtime().block_on(async {
                let url = reqwest::Url::parse("http://fixture.test/secret?token=private").unwrap();
                let checkpoint = Checkpoint::new(&url, &url, "\"v1\"".to_string(), Some(9));
                checkpoint.save(&directory.file()).await.unwrap();
                let file = tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(directory.file())
                    .await
                    .unwrap();
                let mut sink = FaultSink {
                    file,
                    remaining: if flush_failure { None } else { Some(2) },
                    fail_flush: flush_failure,
                };
                let stream = futures_util::stream::iter([Ok::<_, io::Error>(b"abcdef".as_slice())]);
                let mut positions = alloc::vec::Vec::new();
                let result = save_body(
                    stream,
                    &mut sink,
                    &TransferPlan {
                        offset: 3,
                        total: Some(9),
                    },
                    Some(checkpoint),
                    &directory.file(),
                    |n| positions.push(n),
                )
                .await;
                assert!(result.error.is_some());
                let expected = if flush_failure {
                    b"oldabcdef".as_slice()
                } else {
                    b"oldab".as_slice()
                };
                assert_eq!(directory.bytes(), expected);
                assert_eq!(result.size, expected.len() as u64);
                assert_eq!(positions.last(), Some(&result.size));
                assert!(
                    !Checkpoint::load(&directory.file())
                        .await
                        .unwrap()
                        .unwrap()
                        .complete
                );
            });
        }
    }
    #[test]
    fn short_and_oversized_streams_cannot_complete_a_known_representation() {
        for body in [b"ab".as_slice(), b"abcdef".as_slice()] {
            let directory = support::Directory::new();
            runtime().block_on(async {
                let mut file = tokio::fs::File::create(directory.file()).await.unwrap();
                let result = write_body(
                    futures_util::stream::iter([Ok::<_, io::Error>(body)]),
                    &mut file,
                    0,
                    Some(4),
                    |_| {},
                )
                .await;
                assert!(result.error.is_some());
                assert_eq!(result.size, directory.bytes().len() as u64);
                assert!(result.size < 4);
            });
        }
    }
}
