#[cfg(all(feature = "use-dev-tty", not(target_os = "trueos")))]
pub(crate) mod tty;

#[cfg(all(not(feature = "use-dev-tty"), not(target_os = "trueos")))]
pub(crate) mod mio;

#[cfg(all(feature = "use-dev-tty", not(target_os = "trueos")))]
pub(crate) use self::tty::Waker;

#[cfg(all(not(feature = "use-dev-tty"), not(target_os = "trueos")))]
pub(crate) use self::mio::Waker;

#[cfg(target_os = "trueos")]
mod trueos;
#[cfg(target_os = "trueos")]
pub(crate) use self::trueos::Waker;
