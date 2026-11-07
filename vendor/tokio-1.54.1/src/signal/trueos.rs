//! TRUEOS exposes terminal input and native task cancellation, not Unix signals.
//! Registering a Unix signal fails before installing any handlers or pipes.
use crate::platform::io;
use core::task::{Context, Poll};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct SignalKind(libc::c_int);

impl SignalKind {
    /// Allows for listening to any valid OS signal.
    ///
    /// For example, this can be used for listening for platform-specific
    /// signals.
    /// ```rust,no_run
    /// # use tokio::signal::unix::SignalKind;
    /// # let signum = -1;
    /// // let signum = libc::OS_SPECIFIC_SIGNAL;
    /// let kind = SignalKind::from_raw(signum);
    /// ```
    // Use `std::os::raw::c_int` on public API to prevent leaking a non-stable
    // type alias from libc.
    // `libc::c_int` and `std::os::raw::c_int` are currently the same type, and are
    // unlikely to change to other types, but technically libc can change this
    // in the future minor version.
    // See https://github.com/tokio-rs/tokio/issues/3767 for more.
    pub const fn from_raw(signum: std::os::raw::c_int) -> Self {
        Self(signum as libc::c_int)
    }

    /// Get the signal's numeric value.
    ///
    /// ```rust
    /// # use tokio::signal::unix::SignalKind;
    /// let kind = SignalKind::interrupt();
    /// assert_eq!(kind.as_raw_value(), libc::SIGINT);
    /// ```
    pub const fn as_raw_value(&self) -> std::os::raw::c_int {
        self.0
    }

    /// Represents the `SIGALRM` signal.
    ///
    /// On Unix systems this signal is sent when a real-time timer has expired.
    /// By default, the process is terminated by this signal.
    pub const fn alarm() -> Self {
        Self(libc::SIGALRM)
    }

    /// Represents the `SIGCHLD` signal.
    ///
    /// On Unix systems this signal is sent when the status of a child process
    /// has changed. By default, this signal is ignored.
    pub const fn child() -> Self {
        Self(libc::SIGCHLD)
    }

    /// Represents the `SIGHUP` signal.
    ///
    /// On Unix systems this signal is sent when the terminal is disconnected.
    /// By default, the process is terminated by this signal.
    pub const fn hangup() -> Self {
        Self(libc::SIGHUP)
    }

    /// Represents the `SIGINFO` signal.
    ///
    /// On Unix systems this signal is sent to request a status update from the
    /// process. By default, this signal is ignored.
    #[cfg(any(
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "illumos"
    ))]
    pub const fn info() -> Self {
        Self(libc::SIGINFO)
    }

    /// Represents the `SIGINT` signal.
    ///
    /// On Unix systems this signal is sent to interrupt a program.
    /// By default, the process is terminated by this signal.
    pub const fn interrupt() -> Self {
        Self(libc::SIGINT)
    }

    #[cfg(target_os = "haiku")]
    /// Represents the `SIGPOLL` signal.
    ///
    /// On POSIX systems this signal is sent when I/O operations are possible
    /// on some file descriptor. By default, this signal is ignored.
    pub const fn io() -> Self {
        Self(libc::SIGPOLL)
    }
    #[cfg(not(target_os = "haiku"))]
    /// Represents the `SIGIO` signal.
    ///
    /// On Unix systems this signal is sent when I/O operations are possible
    /// on some file descriptor. By default, this signal is ignored.
    pub const fn io() -> Self {
        Self(libc::SIGIO)
    }

    /// Represents the `SIGPIPE` signal.
    ///
    /// On Unix systems this signal is sent when the process attempts to write
    /// to a pipe which has no reader. By default, the process is terminated by
    /// this signal.
    pub const fn pipe() -> Self {
        Self(libc::SIGPIPE)
    }

    /// Represents the `SIGQUIT` signal.
    ///
    /// On Unix systems this signal is sent to issue a shutdown of the
    /// process, after which the OS will dump the process core.
    /// By default, the process is terminated by this signal.
    pub const fn quit() -> Self {
        Self(libc::SIGQUIT)
    }

    /// Represents the `SIGTERM` signal.
    ///
    /// On Unix systems this signal is sent to issue a shutdown of the
    /// process. By default, the process is terminated by this signal.
    pub const fn terminate() -> Self {
        Self(libc::SIGTERM)
    }

    /// Represents the `SIGUSR1` signal.
    ///
    /// On Unix systems this is a user defined signal.
    /// By default, the process is terminated by this signal.
    pub const fn user_defined1() -> Self {
        Self(libc::SIGUSR1)
    }

    /// Represents the `SIGUSR2` signal.
    ///
    /// On Unix systems this is a user defined signal.
    /// By default, the process is terminated by this signal.
    pub const fn user_defined2() -> Self {
        Self(libc::SIGUSR2)
    }

    /// Represents the `SIGWINCH` signal.
    ///
    /// On Unix systems this signal is sent when the terminal window is resized.
    /// By default, this signal is ignored.
    pub const fn window_change() -> Self {
        Self(libc::SIGWINCH)
    }
}

impl From<std::os::raw::c_int> for SignalKind {
    fn from(signum: std::os::raw::c_int) -> Self {
        Self::from_raw(signum as libc::c_int)
    }
}

impl From<SignalKind> for std::os::raw::c_int {
    fn from(kind: SignalKind) -> Self {
        kind.as_raw_value()
    }
}

#[derive(Debug)]
pub struct Signal { never: core::convert::Infallible }

pub fn signal(_kind: SignalKind) -> io::Result<Signal> {
    Err(io::Error::new(io::ErrorKind::Unsupported,
        "Unix signal delivery is unavailable on TRUEOS; use terminal events or native task cancellation"))
}

impl Signal {
    pub async fn recv(&mut self) -> Option<()> { match self.never {} }
    pub fn poll_recv(&mut self, _cx: &mut Context<'_>) -> Poll<Option<()>> { match self.never {} }
}

#[cfg(feature = "process")]
pub(crate) trait InternalStream {
    fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<()>>;
}
#[cfg(feature = "process")]
impl InternalStream for Signal {
    fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<()>> { self.poll_recv(cx) }
}

pub(crate) fn signal_with_handle(_kind: SignalKind,
    _handle: &crate::runtime::signal::Handle) -> io::Result<crate::sync::watch::Receiver<()>> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Unix signal delivery is unavailable on TRUEOS"))
}
pub(crate) fn ctrl_c() -> io::Result<Signal> { signal(SignalKind::interrupt()) }
