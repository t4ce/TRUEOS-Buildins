//! Unix pipe API on TRUEOS. Opening/importing a Unix pipe is unsupported.
//! No native file capability is converted into a descriptor.
use crate::io::{AsyncRead, AsyncWrite, Interest, ReadBuf, Ready};
use crate::platform::io;
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use std::{
    fs::File,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd},
    path::Path,
};
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "TRUEOS has no Unix pipe provider",
    )
}
/// Reject before creating or registering any pipe handle.
pub fn pipe() -> io::Result<(Sender, Receiver)> {
    Err(unsupported())
}
/// Options for an unsupported Unix FIFO operation.
#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    unchecked: bool,
}
impl OpenOptions {
    /// Construct FIFO options.
    pub fn new() -> Self {
        Self::default()
    }
    /// Record the option; opening the FIFO returns `Unsupported`.
    pub fn unchecked(&mut self, value: bool) -> &mut Self {
        self.unchecked = value;
        self
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    /// Opening in read-write mode is also unsupported.
    pub fn read_write(&mut self, _: bool) -> &mut Self {
        self
    }
    /// Return `Unsupported` before touching the path.
    pub fn open_sender<P: AsRef<Path>>(&self, _: P) -> io::Result<Sender> {
        Err(unsupported())
    }
    /// Return `Unsupported` before touching the path.
    pub fn open_receiver<P: AsRef<Path>>(&self, _: P) -> io::Result<Receiver> {
        Err(unsupported())
    }
}
/// A pipe sending end; native TRUEOS cannot construct one.
#[derive(Debug)]
pub enum Sender {}
/// A pipe receiving end; native TRUEOS cannot construct one.
#[derive(Debug)]
pub enum Receiver {}
macro_rules! pipe_end {
    ($end:ty) => {
        impl $end {
            /// Return `Unsupported` without reinterpreting the native file.
            pub fn from_file(_: File) -> io::Result<Self> {
                Err(unsupported())
            }
            /// Return `Unsupported` without registering the descriptor.
            pub fn from_owned_fd(_: OwnedFd) -> io::Result<Self> {
                Err(unsupported())
            }
            /// Return `Unsupported` without reinterpreting the native file.
            pub fn from_file_unchecked(_: File) -> io::Result<Self> {
                Err(unsupported())
            }
            /// Return `Unsupported` without registering the descriptor.
            pub fn from_owned_fd_unchecked(_: OwnedFd) -> io::Result<Self> {
                Err(unsupported())
            }
            /// Native TRUEOS has no pipe handle to wait on.
            pub async fn ready(&self, _: Interest) -> io::Result<Ready> {
                match *self {}
            }
            /// No pipe handle exists, and the closure is never called.
            pub fn try_io<R>(&self, _: impl FnOnce() -> io::Result<R>) -> io::Result<R> {
                match *self {}
            }
            /// No pipe handle exists.
            pub fn into_blocking_fd(self) -> io::Result<OwnedFd> {
                match self {}
            }
            /// No pipe handle exists.
            pub fn into_nonblocking_fd(self) -> io::Result<OwnedFd> {
                match self {}
            }
        }
        // These traits are vacuous: the type has no constructor or stored ID.
        impl AsRawFd for $end {
            fn as_raw_fd(&self) -> RawFd {
                match *self {}
            }
        }
        impl AsFd for $end {
            fn as_fd(&self) -> BorrowedFd<'_> {
                match *self {}
            }
        }
    };
}
pipe_end!(Sender);
pipe_end!(Receiver);
impl Sender {
    /// No pipe handle exists.
    pub async fn writable(&self) -> io::Result<()> {
        match *self {}
    }
    /// No pipe handle exists.
    pub fn poll_write_ready(&self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        match *self {}
    }
    /// No pipe handle exists.
    pub fn try_write(&self, _: &[u8]) -> io::Result<usize> {
        match *self {}
    }
    /// No pipe handle exists.
    pub fn try_write_vectored(&self, _: &[io::IoSlice<'_>]) -> io::Result<usize> {
        match *self {}
    }
}
impl Receiver {
    /// No pipe handle exists.
    pub async fn readable(&self) -> io::Result<()> {
        match *self {}
    }
    /// No pipe handle exists.
    pub fn poll_read_ready(&self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        match *self {}
    }
    /// No pipe handle exists.
    pub fn try_read(&self, _: &mut [u8]) -> io::Result<usize> {
        match *self {}
    }
    /// No pipe handle exists.
    pub fn try_read_vectored(&self, _: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        match *self {}
    }
    cfg_io_util! {
        /// No pipe handle exists.
        pub fn try_read_buf<B: bytes::BufMut>(&self, _: &mut B) -> io::Result<usize> { match *self {} }
    }
}
impl AsyncWrite for Sender {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        match *self {}
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        match *self {}
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        match *self {}
    }
}
impl AsyncRead for Receiver {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match *self {}
    }
}
