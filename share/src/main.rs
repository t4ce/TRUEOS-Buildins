#![cfg_attr(target_os = "trueos", no_std)]
extern crate alloc;

#[cfg(not(target_os = "trueos"))]
use std::env;
#[cfg(not(target_os = "trueos"))]
use std::io;
#[cfg(not(target_os = "trueos"))]
use std::net::SocketAddr;
#[cfg(not(target_os = "trueos"))]
use std::path::PathBuf;

#[cfg(not(target_os = "trueos"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let root = env::args_os().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        env::var_os("TRUEOS_APP_FS_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("share-data"))
    });
    let address: SocketAddr = env::var("TRUEOS_SHARE_BIND")
        .unwrap_or_else(|_| "0.0.0.0:4918".into())
        .parse()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let handler = share::handler(&root).await?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    eprintln!(
        "share: serving {} at http://{}/ (authentication disabled)",
        root.display(),
        listener.local_addr()?
    );
    share::http::serve(listener, handler).await
}

#[cfg(target_os = "trueos")]
fn main() {
    let mut builder = trueos::runtime::current_thread();
    builder.enable_io();
    let runtime = match builder.build() {
        Ok(runtime) => runtime,
        Err(error) => {
            trueos::vsys::log_errorf(format_args!("share: runtime failed: {error}\n"));
            return;
        }
    };
    let result = runtime.block_on(native_serve());
    runtime.shutdown_background();
    if let Err(error) = result {
        trueos::vsys::log_errorf(format_args!("share: {error}\n"));
    }
}

#[cfg(target_os = "trueos")]
async fn native_serve() -> trueos_runtime::io::Result<()> {
    use core::net::SocketAddr;
    use trueos_path::PathBuf;
    use trueos_runtime::io::{Error, ErrorKind};

    let root = trueos::env::args()
        .nth(1)
        .or_else(|| trueos::env::var("TRUEOS_APP_FS_ROOT").ok())
        .unwrap_or_else(|| alloc::string::String::from("apps/share"));
    let root = PathBuf::from(root);
    let address: SocketAddr = trueos::env::var("TRUEOS_SHARE_BIND")
        .unwrap_or_else(|_| alloc::string::String::from("0.0.0.0:4918"))
        .parse()
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "invalid TRUEOS_SHARE_BIND"))?;
    let handler = share::handler(&root).await?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    trueos::vsys::log_infof(format_args!(
        "share: serving {} at http://{}/ (authentication disabled)\n",
        root.as_str(),
        listener.local_addr()?
    ));
    share::http::serve(listener, handler).await
}
