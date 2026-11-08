#[allow(unused_imports)]
use crate::alloc_prelude::*;
#[cfg(all(not(feature = "native"), unix, not(target_os = "trueos")))]
use std::os::fd::AsFd;

#[cfg(all(not(feature = "native"), unix, not(target_os = "trueos")))]
#[allow(unused_variables)]
#[track_caller]
pub(crate) fn check_socket_for_blocking<S: AsFd>(s: &S) -> crate::io::Result<()> {
    #[cfg(not(tokio_allow_from_blocking_fd))]
    {
        let sock = socket2::SockRef::from(s);

        debug_assert!(
            sock.nonblocking()?,
            "Registering a blocking socket with the tokio runtime is unsupported. \
            If you wish to do anyways, please add `--cfg tokio_allow_from_blocking_fd` to your \
            RUSTFLAGS. See github.com/tokio-rs/tokio/issues/7172 for details."
        );
    }

    Ok(())
}

#[cfg(any(feature = "native", not(unix)))]
#[allow(unused_variables)]
pub(crate) fn check_socket_for_blocking<S>(s: &S) -> crate::io::Result<()> {
    // we cannot retrieve the nonblocking status on windows
    // and i don't know how to support wasi yet
    Ok(())
}

#[cfg(all(target_os = "trueos", not(feature = "native")))]
pub(crate) fn check_socket_for_blocking<S>(_: &S) -> crate::io::Result<()> {
    Err(crate::io::Error::new(
        crate::io::ErrorKind::Unsupported,
        "native sockets cannot be imported through Unix descriptor validation",
    ))
}
