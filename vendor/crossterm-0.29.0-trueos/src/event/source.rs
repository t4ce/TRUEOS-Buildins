#[allow(unused_imports)]
use crate::alloc_prelude::*;
use crate::io;
use core::time::Duration;

#[cfg(feature = "event-stream")]
use super::sys::Waker;
use super::InternalEvent;

#[cfg(any(unix, target_os = "trueos"))]
pub(crate) mod unix;
/// Native input identity includes the lease and underlying surface generation,
/// independently of geometry. `event` identifies the source event's identity,
/// even if refreshing `current`
/// has already invalidated the source's buffered bytes.
#[cfg(target_os = "trueos")]
pub(crate) enum NativeInputEpoch {
    Untracked,
    Tracked { current: Option<(u64, u64)>, event: Option<(u64, u64)> },
}

/// An interface for trying to read an `InternalEvent` within an optional `Duration`.
pub(crate) trait EventSource: Sync + Send {
    /// Tries to read an `InternalEvent` within the given duration.
    ///
    /// # Arguments
    ///
    /// * `timeout` - `None` block indefinitely until an event is available, `Some(duration)` blocks
    ///   for the given timeout
    ///
    /// Returns `Ok(None)` if there's no event available and timeout expires.
    fn try_read(&mut self, timeout: Option<Duration>) -> io::Result<Option<InternalEvent>>;

    /// Refresh native input ownership before delivering any queued event.
    #[cfg(target_os = "trueos")]
    fn input_epoch(&mut self) -> io::Result<NativeInputEpoch> {
        Ok(NativeInputEpoch::Untracked)
    }

    /// Clear decoded input and quarantine unfinished control sequences.
    #[cfg(any(unix, target_os = "trueos"))]
    fn discard_buffered_input(&mut self) -> super::InputDiscardStatus;

    /// Replay raw probe input only within its captured native terminal lease.
    #[cfg(target_os = "trueos")]
    fn buffer_input_for_lease(
        &mut self,
        _input: &[u8],
        _lease: u64,
        _events: &mut alloc::collections::VecDeque<InternalEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Other, "terminal source does not support native probe replay"))
    }

    /// Returns a `Waker` allowing to wake/force the `try_read` method to return `Ok(None)`.
    #[cfg(feature = "event-stream")]
    fn waker(&self) -> Waker;
}
