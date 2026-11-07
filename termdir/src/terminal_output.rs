//! Buffered frame output targets the leased TRUEOS console directly.
#[cfg(test)]
use alloc::vec::Vec;

#[cfg(not(target_os = "trueos"))]
pub(crate) use std::io::stdout;

#[cfg(any(target_os = "trueos", test))]
use crossterm::io::{self, Write};

#[cfg(any(target_os = "trueos", test))]
pub(crate) trait Sink {
    fn write(&mut self, bytes: &[u8]) -> usize;
}

#[cfg(any(target_os = "trueos", test))]
pub(crate) struct TerminalOutput<S> {
    sink: S,
}

#[cfg(any(target_os = "trueos", test))]
impl<S> TerminalOutput<S> {
    pub(crate) fn new(sink: S) -> Self {
        Self { sink }
    }
}

#[cfg(any(target_os = "trueos", test))]
impl<S: Sink> Write for TerminalOutput<S> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.sink.write(bytes);
        if count > bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid terminal write count",
            ));
        }
        Ok(count)
    }

    // The attached stream consumes each write synchronously. BufWriter owns
    // frame batching; the native transport has no second userspace buffer.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "trueos")]
pub(crate) struct AttachedConsole;

#[cfg(target_os = "trueos")]
impl Sink for AttachedConsole {
    fn write(&mut self, bytes: &[u8]) -> usize {
        trueos::vshell::attached_write(bytes)
    }
}

#[cfg(target_os = "trueos")]
pub(crate) fn stdout() -> TerminalOutput<AttachedConsole> {
    TerminalOutput::new(AttachedConsole)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffered_output::BufWriter;
    use alloc::rc::Rc;
    use core::cell::RefCell;

    #[derive(Default)]
    struct Capture {
        bytes: Vec<u8>,
        writes: usize,
        max_write: usize,
    }

    struct CaptureSink(Rc<RefCell<Capture>>);

    impl Sink for CaptureSink {
        fn write(&mut self, bytes: &[u8]) -> usize {
            let mut capture = self.0.borrow_mut();
            let count = bytes.len().min(capture.max_write);
            capture.writes += 1;
            capture.bytes.extend_from_slice(&bytes[..count]);
            count
        }
    }

    #[test]
    fn frame_buffer_keeps_one_transport_write_for_a_small_frame() {
        let capture = Rc::new(RefCell::new(Capture {
            max_write: usize::MAX,
            ..Default::default()
        }));
        let mut output =
            BufWriter::with_capacity(64 * 1024, TerminalOutput::new(CaptureSink(capture.clone())));
        for _ in 0..1000 {
            output.write_all(b"\x1b[32mframe").unwrap();
        }
        assert_eq!(capture.borrow().writes, 0);
        output.flush().unwrap();
        assert_eq!(capture.borrow().writes, 1);
        assert_eq!(capture.borrow().bytes.len(), 10_000);
    }

    #[test]
    fn short_native_writes_retain_every_byte_in_order() {
        let capture = Rc::new(RefCell::new(Capture {
            max_write: 3,
            ..Default::default()
        }));
        let mut output =
            BufWriter::with_capacity(64, TerminalOutput::new(CaptureSink(capture.clone())));
        output.write_all(b"0123456789").unwrap();
        output.flush().unwrap();
        assert_eq!(capture.borrow().bytes, b"0123456789");
        assert_eq!(capture.borrow().writes, 4);
    }

    #[test]
    fn unavailable_attached_stream_reports_write_zero() {
        let capture = Rc::new(RefCell::new(Capture::default()));
        let mut output = TerminalOutput::new(CaptureSink(capture));
        assert_eq!(
            output.write_all(b"frame").unwrap_err().kind(),
            io::ErrorKind::WriteZero
        );
    }
}
