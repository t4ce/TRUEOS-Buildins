//! Interactive terminal or headless Hull download worker entrypoint.
#![cfg_attr(target_os = "trueos", no_std)]
extern crate alloc;
#[macro_use]
mod output;
#[cfg(target_os = "trueos")]
mod terminal_line;

use alloc::string::{String, ToString};
#[cfg(not(target_os = "trueos"))]
use std::io::{self, Write};
use trauma::{download::Download, downloader::Downloader};

fn main() {
    if let Err(error) = run() {
        eprintln!("trauma: {error}");
    }
}

fn run() -> Result<(), String> {
    // One native runtime stays alive through terminal parking and reentry.
    #[cfg(target_os = "trueos")]
    let runtime = trueos::runtime::current_thread()
        .build()
        .map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "trueos"))]
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "trueos")]
    if let Ok(script) = trueos::async_fs::block_on(trueos::async_fs::read_file(b"vFile:launch")) {
        if let Some(generation) = trauma::worker::launch_generation(&script)? {
            let result = runtime.block_on(trauma::worker::run(generation));
            runtime.shutdown_background();
            return result;
        }
    }
    #[cfg(target_os = "trueos")]
    let mut lease = trueos::vshell::terminal_initial_lease().map_err(|e| e.to_string())?;
    #[cfg(target_os = "trueos")]
    let mut input = terminal_line::LineReader::new(trueos::io::stdin());
    loop {
        println!("Trauma — enter an HTTP(S) URL; an empty line returns to the shell.");
        #[cfg(not(target_os = "trueos"))]
        io::stdout().flush().map_err(|e| e.to_string())?;
        #[cfg(target_os = "trueos")]
        lease.acknowledge_ready().map_err(|e| e.to_string())?;
        #[cfg(target_os = "trueos")]
        let result = session(&runtime, &mut input);
        #[cfg(not(target_os = "trueos"))]
        let result = session(&runtime);
        #[cfg(target_os = "trueos")]
        let ticket = lease.release_to_shell().map_err(|e| e.to_string())?;
        result?;
        #[cfg(target_os = "trueos")]
        {
            lease = ticket.wait_for_reentry().map_err(|e| e.to_string())?;
        }
        #[cfg(not(target_os = "trueos"))]
        return Ok(());
    }
}

#[cfg(target_os = "trueos")]
fn session(
    runtime: &tokio::runtime::Runtime,
    input: &mut terminal_line::LineReader<trueos::io::Stdin>,
) -> Result<(), String> {
    runtime.block_on(async {
        loop {
            trueos::vsys::log_info("URL> ");
            let Some(line) = input.read_line(trueos::vsys::log_info).await? else {
                return Ok(());
            };
            if line.trim().is_empty() {
                return Ok(());
            }
            let download = Download::builder()
                .url(line.trim())
                .map_err(|e| e.to_string())?
                .build();
            let downloader = Downloader::builder().build();
            for summary in downloader.download(&[download]).await {
                println!(
                    "{:?}; HTTP {}; {} bytes committed",
                    summary.status(),
                    summary.statuscode().as_u16(),
                    summary.size()
                );
            }
        }
    })
}

#[cfg(not(target_os = "trueos"))]
fn session(runtime: &tokio::runtime::Runtime) -> Result<(), String> {
    loop {
        print!("URL> ");
        io::stdout().flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        if io::stdin()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?
            == 0
            || line.trim().is_empty()
        {
            return Ok(());
        }
        let download = Download::builder()
            .url(line.trim())
            .map_err(|e| e.to_string())?
            .build();
        let downloader = Downloader::builder().build();
        for summary in runtime.block_on(downloader.download(&[download])) {
            println!(
                "{:?}; HTTP {}; {} bytes committed",
                summary.status(),
                summary.statuscode().as_u16(),
                summary.size()
            );
        }
    }
}
