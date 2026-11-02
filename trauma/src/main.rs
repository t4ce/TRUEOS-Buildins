//! Interactive terminal or headless Hull download worker entrypoint.
extern crate alloc;

#[cfg(any(target_os = "trueos", target_os = "zkvm"))]
use alloc::string::ToString;
use alloc::{boxed::Box, string::String};
use std::io::{self, Write};
use trauma::{download::Download, downloader::Downloader};

fn main() {
    if let Err(error) = run() {
        eprintln!("trauma: {error}");
    }
}

fn run() -> Result<(), Box<dyn core::error::Error>> {
    // Keep one runtime alive across terminal parking and reentry.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    #[cfg(any(target_os = "trueos", target_os = "zkvm"))]
    if let Ok(script) = trueos::async_fs::block_on(trueos::async_fs::read_file(b"vFile:launch")) {
        if let Some(generation) =
            trauma::worker::launch_generation(&script).map_err(io::Error::other)?
        {
            return runtime
                .block_on(trauma::worker::run(generation))
                .map_err(|e| io::Error::other(e).into());
        }
    }
    #[cfg(any(target_os = "trueos", target_os = "zkvm"))]
    let mut lease = trueos::vshell::terminal_initial_lease()
        .map_err(|error| io::Error::other(error.to_string()))?;

    loop {
        println!("Trauma — enter an HTTP(S) URL; an empty line returns to the shell.");
        io::stdout().flush()?;
        #[cfg(any(target_os = "trueos", target_os = "zkvm"))]
        lease
            .acknowledge_ready()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let result = session(&runtime);
        #[cfg(any(target_os = "trueos", target_os = "zkvm"))]
        let ticket = lease
            .release_to_shell()
            .map_err(|error| io::Error::other(error.to_string()))?;
        result?;
        #[cfg(any(target_os = "trueos", target_os = "zkvm"))]
        {
            lease = ticket
                .wait_for_reentry()
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        #[cfg(not(any(target_os = "trueos", target_os = "zkvm")))]
        return Ok(());
    }
}

fn session(runtime: &tokio::runtime::Runtime) -> Result<(), Box<dyn core::error::Error>> {
    loop {
        print!("URL> ");
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 || line.trim().is_empty() {
            return Ok(());
        }
        let download = Download::builder().url(line.trim())?.build();
        // Use the same Download/Downloader flow as examples/simple.rs.
        let downloader = Downloader::builder().build();
        for summary in runtime.block_on(downloader.download(&[download])) {
            println!("{summary:?}");
        }
    }
}
