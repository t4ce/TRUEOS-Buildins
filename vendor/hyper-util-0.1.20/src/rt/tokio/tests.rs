use super::*;
use core::fmt;
use core::task::Waker;

#[derive(Debug)]
struct SourceError;

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("original device error")
    }
}

impl core::error::Error for SourceError {}

struct Probe {
    error: Option<tokio_io::Error>,
    slices: Option<(*const (), usize)>,
}

impl Probe {
    fn failing(error: tokio_io::Error) -> Self {
        Self {
            error: Some(error),
            slices: None,
        }
    }

    fn vectored(bufs: &[IoSlice<'_>]) -> Self {
        Self {
            error: None,
            slices: Some((bufs.as_ptr().cast(), bufs.len())),
        }
    }

    fn error(&mut self) -> tokio_io::Error {
        self.error.take().expect("original error consumed once")
    }

    fn check_slices(&self, bufs: &[IoSlice<'_>]) -> usize {
        let (pointer, len) = self.slices.expect("vectored probe");
        assert_eq!(
            bufs.as_ptr().cast(),
            pointer,
            "forward the original descriptor array"
        );
        assert_eq!(bufs.len(), len);
        bufs.iter().map(|buf| buf.len()).sum()
    }
}

impl tokio_io::AsyncRead for Probe {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut tokio_io::ReadBuf<'_>,
    ) -> Poll<tokio_io::Result<()>> {
        Poll::Ready(Err(self.error()))
    }
}

impl tokio_io::AsyncWrite for Probe {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &[u8],
    ) -> Poll<tokio_io::Result<usize>> {
        Poll::Ready(Err(self.error()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<tokio_io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<tokio_io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<tokio_io::Result<usize>> {
        Poll::Ready(Ok(self.check_slices(bufs)))
    }

    fn is_write_vectored(&self) -> bool {
        true
    }
}

impl hyper::rt::Read for Probe {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<hyper_io::Result<()>> {
        Poll::Ready(Err(self.error()))
    }
}

impl hyper::rt::Write for Probe {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &[u8],
    ) -> Poll<hyper_io::Result<usize>> {
        Poll::Ready(Err(self.error()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<hyper_io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<hyper_io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bufs: &[hyper_io::IoSlice<'_>],
    ) -> Poll<hyper_io::Result<usize>> {
        Poll::Ready(Ok(self.check_slices(bufs)))
    }

    fn is_write_vectored(&self) -> bool {
        true
    }
}

fn hyper_write_error(mut io: impl hyper::rt::Write + Unpin) -> tokio_io::Error {
    let mut cx = Context::from_waker(Waker::noop());
    match Pin::new(&mut io).poll_write(&mut cx, b"test") {
        Poll::Ready(Err(error)) => error,
        _ => panic!("expected original error"),
    }
}

fn tokio_write_error(mut io: impl tokio_io::AsyncWrite + Unpin) -> tokio_io::Error {
    let mut cx = Context::from_waker(Waker::noop());
    match Pin::new(&mut io).poll_write(&mut cx, b"test") {
        Poll::Ready(Err(error)) => error,
        _ => panic!("expected original error"),
    }
}

#[test]
fn adapters_preserve_os_errors() {
    let adapters: [fn(tokio_io::Error) -> tokio_io::Error; 4] = [
        |err| hyper_write_error(TokioIo::new(Probe::failing(err))),
        |err| tokio_write_error(TokioIo::new(Probe::failing(err))),
        |err| hyper_write_error(WithHyperIo::new(Probe::failing(err))),
        |err| tokio_write_error(WithTokioIo::new(Probe::failing(err))),
    ];
    for adapt in adapters {
        let original = tokio_io::Error::from_raw_os_error(2);
        let kind = original.kind();
        let error = adapt(original);
        assert_eq!(error.raw_os_error(), Some(2));
        assert_eq!(error.kind(), kind);
    }
}

#[test]
fn adapters_preserve_custom_read_errors() {
    let adapters: [fn(tokio_io::Error) -> tokio_io::Error; 4] = [
        |err| hyper_read_error(TokioIo::new(Probe::failing(err))),
        |err| tokio_read_error(TokioIo::new(Probe::failing(err))),
        |err| hyper_read_error(WithHyperIo::new(Probe::failing(err))),
        |err| tokio_read_error(WithTokioIo::new(Probe::failing(err))),
    ];
    for adapt in adapters {
        let error = adapt(tokio_io::Error::new(
            tokio_io::ErrorKind::Unsupported,
            SourceError,
        ));
        assert_eq!(error.kind(), tokio_io::ErrorKind::Unsupported);
        assert!(error.get_ref().unwrap().is::<SourceError>());
        assert_eq!(error.to_string(), "original device error");
    }
}

fn hyper_read_error(mut io: impl hyper::rt::Read + Unpin) -> tokio_io::Error {
    let mut cx = Context::from_waker(Waker::noop());
    let mut bytes = [0; 4];
    let mut buf = hyper::rt::ReadBuf::new(&mut bytes);
    match Pin::new(&mut io).poll_read(&mut cx, buf.unfilled()) {
        Poll::Ready(Err(error)) => error,
        _ => panic!("expected original error"),
    }
}

fn tokio_read_error(mut io: impl tokio_io::AsyncRead + Unpin) -> tokio_io::Error {
    let mut cx = Context::from_waker(Waker::noop());
    let mut bytes = [0; 4];
    let mut buf = tokio_io::ReadBuf::new(&mut bytes);
    match Pin::new(&mut io).poll_read(&mut cx, &mut buf) {
        Poll::Ready(Err(error)) => error,
        _ => panic!("expected original error"),
    }
}

#[test]
fn adapters_borrow_vectored_descriptors() {
    let bufs = [IoSlice::new(b"first"), IoSlice::new(b"second")];
    let mut cx = Context::from_waker(Waker::noop());
    let mut io = TokioIo::new(Probe::vectored(&bufs));
    assert!(matches!(
        hyper::rt::Write::poll_write_vectored(Pin::new(&mut io), &mut cx, &bufs),
        Poll::Ready(Ok(11))
    ));
    let mut io = TokioIo::new(Probe::vectored(&bufs));
    assert!(matches!(
        tokio_io::AsyncWrite::poll_write_vectored(Pin::new(&mut io), &mut cx, &bufs),
        Poll::Ready(Ok(11))
    ));
    let mut io = WithHyperIo::new(Probe::vectored(&bufs));
    assert!(matches!(
        hyper::rt::Write::poll_write_vectored(Pin::new(&mut io), &mut cx, &bufs),
        Poll::Ready(Ok(11))
    ));
    let mut io = WithTokioIo::new(Probe::vectored(&bufs));
    assert!(matches!(
        tokio_io::AsyncWrite::poll_write_vectored(Pin::new(&mut io), &mut cx, &bufs),
        Poll::Ready(Ok(11))
    ));
}

#[tokio::test(start_paused = true)]
async fn timer_preserves_deadlines_and_paused_clock() {
    tokio::time::advance(Duration::from_secs(10)).await;
    let timer = TokioTimer::new();
    let now = tokio::time::Instant::now();
    assert_eq!(timer.now(), now.into_std());
    for deadline in [
        now - Duration::from_secs(1),
        now,
        now + Duration::from_secs(5),
    ] {
        assert_eq!(
            tokio_to_hyper_instant(hyper_to_tokio_instant(deadline.into_std())),
            deadline.into_std()
        );
        let mut sleep = timer.sleep_until(deadline.into_std());
        let inner = sleep.as_mut().downcast_mut_pin::<TokioSleep>().unwrap();
        assert_eq!(inner.as_ref().get_ref().inner.deadline(), deadline);
        let new_deadline = deadline + Duration::from_secs(3);
        timer.reset(&mut sleep, new_deadline.into_std());
        let inner = sleep.as_mut().downcast_mut_pin::<TokioSleep>().unwrap();
        assert_eq!(inner.as_ref().get_ref().inner.deadline(), new_deadline);
    }
}
