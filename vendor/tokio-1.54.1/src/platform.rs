//! Runtime platform selection. Native mode links no Rust std or Unix API.
#[cfg(not(feature = "native"))]
pub(crate) use std::{env, io, sync, thread, time};
#[cfg(not(feature = "native"))]
pub(crate) mod panic {
    pub(crate) use std::panic::catch_unwind as task_boundary;
    pub(crate) use std::panic::*;
}
#[cfg(not(feature = "native"))]
pub(crate) use std::process::abort;
#[cfg(feature = "native")]
pub(crate) use trueos_runtime::{abort, env, sync, thread};
#[cfg(feature = "native")]
pub(crate) mod io {
    pub use trueos_runtime::io::*;
    #[cfg(feature = "io-std")]
    pub(crate) use crate::io::native_stdio_backend::{stdin, stdout, stderr, Stdin, Stdout, Stderr};
}
#[cfg(feature = "native")]
pub(crate) mod time {
    pub(crate) use ::core::time::Duration;
    pub(crate) use trueos_runtime::time::Instant;
}
#[cfg(feature = "native")]
pub(crate) mod panic {
    pub(crate) use core::panic::*;
    // Native panic aborts. A task cannot unwind across this boundary, so normal
    // completion is Ok; panics terminate the process through its panic handler.
    pub(crate) fn task_boundary<F: FnOnce() -> R + UnwindSafe, R>(
        f: F,
    ) -> Result<R, alloc::boxed::Box<dyn ::core::any::Any + Send>> {
        Ok(f())
    }
    pub(crate) fn resume_unwind(_: alloc::boxed::Box<dyn ::core::any::Any + Send>) -> ! {
        trueos_runtime::abort()
    }
}
