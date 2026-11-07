use crate::{Result, terminal::POLL};
use alloc::{format, string::String};
use core::time::Duration;
#[cfg(not(target_os = "trueos"))]
use std::io::ErrorKind;
use tokio::{net::TcpStream, time::Instant};
#[cfg(target_os = "trueos")]
use trueos_runtime::io::ErrorKind;

// The native pump runs on its current-thread runtime.
#[allow(async_fn_in_trait)]
pub trait Wire {
    /// None is temporary unavailability; Some(0) is received EOF.
    fn read(&mut self, out: &mut [u8]) -> Result<Option<usize>>;
    fn write(&mut self, bytes: &[u8]) -> Result<Option<usize>>;
    async fn wait(&mut self, read: bool, write: bool) -> Result<()>;
    async fn shutdown(&mut self) -> Result<()>;
}
pub struct TcpWire(pub TcpStream);
impl TcpWire {
    pub async fn connect(host: &str, port: u16, deadline: Instant) -> Result<Self> {
        tokio::time::timeout_at(deadline, TcpStream::connect((host, port)))
            .await
            .map_err(|_| String::from("ssh: connect timed out after15 seconds"))?
            .map(Self)
            .map_err(|e| format!("ssh: connect failed: {e}"))
    }
}
impl Wire for TcpWire {
    fn read(&mut self, out: &mut [u8]) -> Result<Option<usize>> {
        match self.0.try_read(out) {
            Ok(n) => Ok(Some(n)),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                Ok(None)
            }
            Err(e) => Err(format!("ssh: TCP read failed: {e}")),
        }
    }
    fn write(&mut self, bytes: &[u8]) -> Result<Option<usize>> {
        match self.0.try_write(bytes) {
            Ok(0) if !bytes.is_empty() => Err("ssh: TCP stopped accepting output".into()),
            Ok(n) => Ok(Some(n)),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                Ok(None)
            }
            Err(e) => Err(format!("ssh: TCP write failed: {e}")),
        }
    }
    async fn wait(&mut self, read: bool, write: bool) -> Result<()> {
        let interest = match (read, write) {
            (true, true) => Some(tokio::io::Interest::READABLE | tokio::io::Interest::WRITABLE),
            (true, false) => Some(tokio::io::Interest::READABLE),
            (false, true) => Some(tokio::io::Interest::WRITABLE),
            (false, false) => None,
        };
        if let Some(interest) = interest {
            tokio::select! {result=self.0.ready(interest)=>{result.map_err(|e|format!("ssh: TCP readiness failed: {e}"))?;},()=tokio::time::sleep(POLL)=>{}}
        } else {
            tokio::time::sleep(POLL).await
        }
        Ok(())
    }
    async fn shutdown(&mut self) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        tokio::time::timeout(Duration::from_secs(1), self.0.shutdown())
            .await
            .map_err(|_| "ssh: TCP close timed out")?
            .map_err(|e| format!("ssh: TCP close failed: {e}"))
    }
}
