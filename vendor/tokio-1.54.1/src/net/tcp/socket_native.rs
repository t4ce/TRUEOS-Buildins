//! Native unconnected socket ownership. No file descriptor or std socket bridge.
use crate::{
    net::{TcpListener, TcpStream},
    platform::io,
};
use core::{net::SocketAddr, time::Duration};

/// An owned, initially unconnected TCP socket in the native network service.
#[derive(Debug)]
pub struct TcpSocket {
    inner: mio::net::TcpSocket,
}
impl TcpSocket {
    /// Allocate an IPv4 socket in the native network service.
    pub fn new_v4() -> io::Result<Self> {
        Ok(Self {
            inner: mio::net::TcpSocket::new_v4()?,
        })
    }
    /// Allocate an IPv6 socket if the native network service supports IPv6.
    pub fn new_v6() -> io::Result<Self> {
        Ok(Self {
            inner: mio::net::TcpSocket::new_v6()?,
        })
    }
    /// Reserve a local endpoint now, before connect or listen.
    pub fn bind(&self, addr: SocketAddr) -> io::Result<()> {
        self.inner.bind(addr)
    }
    /// Return the reserved local endpoint.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
    /// Take the pending native socket error.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
    }
    /// Connect using the reserved endpoint, then await the reactor result.
    pub async fn connect(self, addr: SocketAddr) -> io::Result<TcpStream> {
        TcpStream::connect_mio(self.inner.connect(addr)?).await
    }
    /// Listen using this socket's endpoint reservation and requested backlog.
    ///
    /// The current native service accepts backlogs through 32. Zero requests
    /// one slot; larger values return an error without silently reducing it.
    pub fn listen(self, backlog: u32) -> io::Result<TcpListener> {
        TcpListener::new(self.inner.listen(backlog)?)
    }
    /// Set the Nagle policy on the actual native socket.
    pub fn set_nodelay(&self, value: bool) -> io::Result<()> {
        self.inner.set_nodelay(value)
    }
    /// Query the Nagle policy on the actual native socket.
    pub fn nodelay(&self) -> io::Result<bool> {
        self.inner.nodelay()
    }
    /// Native keepalive configuration is currently unsupported.
    pub fn set_keepalive(&self, _: bool) -> io::Result<()> {
        Err(unsupported("native TCP keepalive is unsupported"))
    }
    /// Native keepalive configuration is currently unsupported.
    pub fn keepalive(&self) -> io::Result<bool> {
        Err(unsupported("native TCP keepalive is unsupported"))
    }
    /// Native address reuse policy is currently unsupported.
    pub fn set_reuseaddr(&self, _: bool) -> io::Result<()> {
        Err(unsupported("native TCP address reuse is unsupported"))
    }
    /// Native address reuse policy is currently unsupported.
    pub fn reuseaddr(&self) -> io::Result<bool> {
        Err(unsupported("native TCP address reuse is unsupported"))
    }
    /// Native shared-port policy is currently unsupported.
    pub fn set_reuseport(&self, _: bool) -> io::Result<()> {
        Err(unsupported("native TCP shared ports are unsupported"))
    }
    /// Native shared-port policy is currently unsupported.
    pub fn reuseport(&self) -> io::Result<bool> {
        Err(unsupported("native TCP shared ports are unsupported"))
    }
    /// Native buffer resizing is currently unsupported.
    pub fn set_send_buffer_size(&self, _: u32) -> io::Result<()> {
        Err(unsupported("native TCP buffer resizing is unsupported"))
    }
    /// Native buffer-size queries are currently unsupported.
    pub fn send_buffer_size(&self) -> io::Result<u32> {
        Err(unsupported("native TCP buffer-size query is unsupported"))
    }
    /// Native buffer resizing is currently unsupported.
    pub fn set_recv_buffer_size(&self, _: u32) -> io::Result<()> {
        Err(unsupported("native TCP buffer resizing is unsupported"))
    }
    /// Native buffer-size queries are currently unsupported.
    pub fn recv_buffer_size(&self) -> io::Result<u32> {
        Err(unsupported("native TCP buffer-size query is unsupported"))
    }
    /// Native linger configuration is currently unsupported.
    #[deprecated = "linger can block a socket close"]
    pub fn set_linger(&self, _: Option<Duration>) -> io::Result<()> {
        Err(unsupported("native TCP linger is unsupported"))
    }
    /// Native abortive-close configuration is currently unsupported.
    pub fn set_zero_linger(&self) -> io::Result<()> {
        Err(unsupported("native TCP abortive close is unsupported"))
    }
    /// Native linger configuration is currently unsupported.
    pub fn linger(&self) -> io::Result<Option<Duration>> {
        Err(unsupported("native TCP linger is unsupported"))
    }
}
fn unsupported(detail: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, detail)
}
