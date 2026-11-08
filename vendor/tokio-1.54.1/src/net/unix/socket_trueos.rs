//! Native TRUEOS does not offer Unix-domain socket construction or FD import.
use crate::platform::io;
use crate::net::{UnixDatagram, UnixListener, UnixStream};
use std::path::Path;
/// An unavailable Unix-domain socket. Native TCP/UDP use their typed providers.
#[derive(Debug)]
pub enum UnixSocket {}
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "TRUEOS native networking has no Unix-domain socket or descriptor import provider",
    )
}
impl UnixSocket {
    /// Unix-domain datagram creation is unavailable on the native transport.
    pub fn new_datagram() -> io::Result<Self> {
        Err(unsupported())
    }
    /// Unix-domain stream creation is unavailable on the native transport.
    pub fn new_stream() -> io::Result<Self> {
        Err(unsupported())
    }
    /// There is no constructed native Unix-domain socket to bind.
    pub fn bind(&self, _path: impl AsRef<Path>) -> io::Result<()> {
        match *self {}
    }
    /// There is no constructed native Unix-domain socket to listen on.
    pub fn listen(self, _backlog: u32) -> io::Result<UnixListener> {
        match self {}
    }
    /// There is no constructed native Unix-domain socket to connect.
    pub async fn connect(self, _path: impl AsRef<Path>) -> io::Result<UnixStream> {
        match self {}
    }
    /// There is no constructed native Unix-domain datagram capability.
    pub fn datagram(self) -> io::Result<UnixDatagram> {
        match self {}
    }
}
