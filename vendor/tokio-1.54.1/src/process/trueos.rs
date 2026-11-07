//! The native process model has no Unix argv/stdio child provider.
//! Public fallible conversions reject before subscribing to SIGCHLD or using
//! a descriptor. Actual Blueprint launches use the native launch API.
use crate::io::{AsyncRead, AsyncWrite, ReadBuf};
use crate::platform::io;
use crate::process::{SpawnedChild, kill::Kill};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::process::{
    Child as StdChild, ChildStderr as StdChildStderr, ChildStdin as StdChildStdin,
    ChildStdout as StdChildStdout, ExitStatus, Stdio,
};

pub(crate) fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "TRUEOS native processes do not provide Unix child stdio",
    )
}

#[derive(Debug)]
pub(crate) enum Child {}
impl Child {
    pub(crate) fn id(&self) -> u32 {
        match *self {}
    }
    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match *self {}
    }
}
impl Kill for Child {
    fn kill(&mut self) -> io::Result<()> {
        match *self {}
    }
}
impl Future for Child {
    type Output = io::Result<ExitStatus>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        match *self {}
    }
}
pub(crate) fn build_child(_: StdChild) -> io::Result<SpawnedChild> {
    Err(unsupported())
}

#[derive(Debug)]
pub(crate) enum ChildStdin {}
#[derive(Debug)]
pub(crate) enum ChildStdout {}
pub(crate) type ChildStderr = ChildStdout;
macro_rules! child_io {
    ($end:ty) => {
        impl $end {
            pub(super) fn into_owned_fd(self) -> io::Result<OwnedFd> {
                match self {}
            }
            pub(super) fn into_stdio(self) -> io::Result<Stdio> {
                match self {}
            }
        }
        // Vacuous trait implementation: there is no constructor or stored ID.
        impl AsRawFd for $end {
            fn as_raw_fd(&self) -> RawFd {
                match *self {}
            }
        }
    };
}
child_io!(ChildStdin);
child_io!(ChildStdout);
impl TryFrom<StdChildStdin> for ChildStdin {
    type Error = io::Error;
    fn try_from(_: StdChildStdin) -> io::Result<Self> {
        Err(unsupported())
    }
}
impl TryFrom<StdChildStdout> for ChildStdout {
    type Error = io::Error;
    fn try_from(_: StdChildStdout) -> io::Result<Self> {
        Err(unsupported())
    }
}
impl TryFrom<StdChildStderr> for ChildStdout {
    type Error = io::Error;
    fn try_from(_: StdChildStderr) -> io::Result<Self> {
        Err(unsupported())
    }
}
impl AsyncWrite for ChildStdin {
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
impl AsyncRead for ChildStdout {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match *self {}
    }
}
pub(super) fn stdio<T, U>(value: T) -> io::Result<U>
where
    U: TryFrom<T, Error = io::Error>,
{
    U::try_from(value)
}
