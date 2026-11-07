#[allow(unused_imports)]
use crate::alloc_prelude::*;
mod parser;

#[cfg(all(feature = "use-dev-tty", not(target_os = "trueos")))]
pub(crate) mod tty;

#[cfg(all(not(feature = "use-dev-tty"), not(target_os = "trueos")))]
pub(crate) mod mio;

#[cfg(all(feature = "use-dev-tty", not(target_os = "trueos")))]
pub(crate) use self::tty::UnixInternalEventSource;

#[cfg(all(not(feature = "use-dev-tty"), not(target_os = "trueos")))]
pub(crate) use self::mio::UnixInternalEventSource;

// TRUEOS terminals are attached byte streams with typed surface generations.
// No descriptor registry, termios, or signal pipe is needed for this source.
#[cfg(any(target_os = "trueos", all(test, not(feature = "event-stream"))))]
mod trueos;
#[cfg(target_os = "trueos")]
pub(crate) use self::trueos::NativeInternalEventSource as UnixInternalEventSource;
