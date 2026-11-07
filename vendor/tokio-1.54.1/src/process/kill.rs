#[allow(unused_imports)]
use crate::alloc_prelude::*;
use crate::platform::io;

/// An interface for killing a running process.
pub(crate) trait Kill {
    /// Forcefully kills the process.
    fn kill(&mut self) -> io::Result<()>;
}

impl<T: Kill> Kill for &mut T {
    fn kill(&mut self) -> io::Result<()> {
        (**self).kill()
    }
}
