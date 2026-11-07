//! Frame batching over the shared native Write trait, including short-write recovery.
use alloc::vec::Vec;
#[cfg(test)]
use core::fmt;
use crossterm::io::{self, Write};

pub(crate) struct BufWriter<W: Write> {
    inner: Option<W>,
    buffer: Vec<u8>,
    panicked: bool,
}
impl<W: Write> BufWriter<W> {
    pub(crate) fn with_capacity(capacity: usize, inner: W) -> Self {
        Self {
            inner: Some(inner),
            buffer: Vec::with_capacity(capacity),
            panicked: false,
        }
    }

    fn flush_buffer(&mut self) -> io::Result<()> {
        let mut written = 0;
        let result = loop {
            if written == self.buffer.len() {
                break Ok(());
            }
            // An unwinding inner writer may already have consumed bytes. Never
            // replay them from Drop when the write did not report its count.
            self.panicked = true;
            let result = self.inner.as_mut().unwrap().write(&self.buffer[written..]);
            self.panicked = false;
            match result {
                Ok(0) => {
                    break Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "frame output made no progress",
                    ));
                }
                Ok(count) if count <= self.buffer.len() - written => written += count,
                Ok(_) => {
                    break Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid frame output write count",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => break Err(error),
            }
        };
        self.buffer.drain(..written);
        result
    }

    #[cfg(test)]
    pub(crate) fn into_inner(mut self) -> core::result::Result<W, IntoInnerError<W>> {
        if let Err(error) = self.flush_buffer() {
            return Err(IntoInnerError {
                writer: self,
                error,
            });
        }
        Ok(self.inner.take().unwrap())
    }
}
impl<W: Write> Write for BufWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.buffer.capacity() - self.buffer.len() {
            self.flush_buffer()?;
        }
        if bytes.len() >= self.buffer.capacity() {
            self.panicked = true;
            let result = self.inner.as_mut().unwrap().write(bytes);
            self.panicked = false;
            result
        } else {
            self.buffer.extend_from_slice(bytes);
            Ok(bytes.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flush_buffer()?;
        self.inner.as_mut().unwrap().flush()
    }
}
impl<W: Write> Drop for BufWriter<W> {
    fn drop(&mut self) {
        if self.inner.is_some() && !self.panicked {
            let _ = self.flush_buffer();
        }
    }
}
#[cfg(test)]
pub(crate) struct IntoInnerError<W: Write> {
    writer: BufWriter<W>,
    error: io::Error,
}
#[cfg(test)]
impl<W: Write> fmt::Debug for IntoInnerError<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IntoInnerError").field(&self.error).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::{rc::Rc, vec};
    use core::cell::RefCell;
    use std::io::Write as _;

    #[derive(Default, Debug, PartialEq, Eq)]
    struct Trace {
        calls: Vec<Vec<u8>>,
        bytes: Vec<u8>,
        flushes: usize,
    }
    struct Sink {
        trace: Rc<RefCell<Trace>>,
        max: usize,
        fail_after: Option<usize>,
        interrupt: bool,
    }
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.trace.borrow_mut().calls.push(bytes.to_vec());
            if self.interrupt {
                self.interrupt = false;
                return Err(io::Error::new(io::ErrorKind::Interrupted, "retry"));
            }
            let consumed = self.trace.borrow().bytes.len();
            if self.fail_after == Some(consumed) {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "paused"));
            }
            let n = bytes
                .len()
                .min(self.max)
                .min(self.fail_after.map_or(usize::MAX, |limit| limit - consumed));
            self.trace.borrow_mut().bytes.extend_from_slice(&bytes[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.trace.borrow_mut().flushes += 1;
            Ok(())
        }
    }
    // A real trait adapter allows this same production writer to be checked
    // against std while crossterm selects either std or native core3 I/O.
    struct StdSink(Sink);
    impl std::io::Write for StdSink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .write(bytes)
                .map_err(|error| std::io::Error::other(error.to_string()))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0
                .flush()
                .map_err(|error| std::io::Error::other(error.to_string()))
        }
    }
    #[test]
    fn batching_and_short_write_trace_matches_std() {
        for capacity in [0, 1, 8, 64 * 1024] {
            for max in [1, 3, usize::MAX] {
                let a = Rc::new(RefCell::new(Trace::default()));
                let b = Rc::new(RefCell::new(Trace::default()));
                let sink = |trace| Sink {
                    trace,
                    max,
                    fail_after: None,
                    interrupt: false,
                };
                let mut native = BufWriter::with_capacity(capacity, sink(a.clone()));
                let mut old = std::io::BufWriter::with_capacity(capacity, StdSink(sink(b.clone())));
                for fragment in [
                    b"ab".as_slice(),
                    b"",
                    b"0123456789",
                    b"frame",
                    b"\x1b[32m",
                    b"x",
                ] {
                    native.write_all(fragment).unwrap();
                    old.write_all(fragment).unwrap();
                }
                native.flush().unwrap();
                old.flush().unwrap();
                assert_eq!(*a.borrow(), *b.borrow(), "capacity={capacity}, short={max}");
            }
        }
    }
    #[test]
    fn interrupted_and_partial_failure_preserve_unwritten_suffix() {
        let trace = Rc::new(RefCell::new(Trace::default()));
        let mut output = BufWriter::with_capacity(
            32,
            Sink {
                trace: trace.clone(),
                max: 3,
                fail_after: Some(5),
                interrupt: true,
            },
        );
        output.write_all(b"0123456789").unwrap();
        assert_eq!(
            output.flush().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(trace.borrow().bytes, b"01234");
        assert_eq!(output.buffer, b"56789");
        output.inner.as_mut().unwrap().fail_after = None;
        output.flush().unwrap();
        assert_eq!(trace.borrow().bytes, b"0123456789");
        assert_eq!(trace.borrow().flushes, 1);
    }
    #[test]
    fn failed_into_inner_preserves_buffer_and_writer_for_retry() {
        let trace = Rc::new(RefCell::new(Trace::default()));
        let mut output = BufWriter::with_capacity(
            32,
            Sink {
                trace: trace.clone(),
                max: 0,
                fail_after: None,
                interrupt: false,
            },
        );
        output.write_all(b"frame").unwrap();
        let mut error = output.into_inner().err().unwrap();
        assert_eq!(error.error.kind(), io::ErrorKind::WriteZero);
        error.writer.inner.as_mut().unwrap().max = 2;
        error.writer.flush().unwrap();
        assert_eq!(trace.borrow().bytes, b"frame");
    }
    #[test]
    fn unwinding_transport_never_replays_unreported_bytes_from_drop() {
        struct Panicking(Rc<RefCell<usize>>);
        impl Write for Panicking {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                *self.0.borrow_mut() += 1;
                panic!("transport panicked after consuming bytes");
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let calls = Rc::new(RefCell::new(0));
        let mut output = BufWriter::with_capacity(32, Panicking(calls.clone()));
        output.write_all(b"frame").unwrap();
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| output.flush())).is_err());
        drop(output);
        assert_eq!(*calls.borrow(), 1);
    }
    #[test]
    fn drop_flushes_buffer_without_flushing_inner_transport() {
        let trace = Rc::new(RefCell::new(Trace::default()));
        {
            let mut output = BufWriter::with_capacity(
                32,
                Sink {
                    trace: trace.clone(),
                    max: usize::MAX,
                    fail_after: None,
                    interrupt: false,
                },
            );
            output.write_all(b"frame").unwrap();
        }
        assert_eq!(trace.borrow().calls, vec![b"frame".to_vec()]);
        assert_eq!(trace.borrow().flushes, 0);
    }
}
