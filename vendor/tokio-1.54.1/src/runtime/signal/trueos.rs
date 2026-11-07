//! TRUEOS has no Unix signal pipe or child-reaper driver.
//! The runtime retains its ordinary I/O parking path; subscribing to a Unix
//! signal reports Unsupported at the public signal boundary.
use crate::{platform::io, runtime::{driver, io as runtime_io}};
use core::time::Duration;

#[derive(Debug)]
pub(crate) struct Driver { io: runtime_io::Driver }

#[derive(Debug, Default)]
pub(crate) struct Handle;

impl Driver {
    pub(crate) fn new(io: runtime_io::Driver, _handle: &runtime_io::Handle) -> io::Result<Self> {
        Ok(Self { io })
    }
    pub(crate) fn handle(&self) -> Handle { Handle }
    pub(crate) fn park(&mut self, handle: &driver::Handle) { self.io.park(handle); }
    pub(crate) fn park_timeout(&mut self, handle: &driver::Handle, duration: Duration) {
        self.io.park_timeout(handle, duration);
    }
    pub(crate) fn shutdown(&mut self, handle: &driver::Handle) { self.io.shutdown(handle); }
}
impl Handle {
    pub(crate) fn check_inner(&self) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Unix signal delivery is unavailable on TRUEOS"))
    }
}
