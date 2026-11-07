//! Native resident SSH client, sharing the IO-less Sunset engine with the kernel.
#![cfg_attr(target_os = "trueos", no_std)]
extern crate alloc;
use alloc::{format, string::String, vec::Vec};
use trueos_ssh_client::{
    Result,
    options::{self, Options},
    platform_store::NativeStore,
    session, stage_log,
    terminal::{self, PlatformConsole, Terminal},
};
fn process_args() -> Vec<String> {
    #[cfg(target_os = "trueos")]
    {
        trueos::env::args().collect()
    }
    #[cfg(not(target_os = "trueos"))]
    {
        std::env::args().collect()
    }
}
fn main() {
    stage_log("main-enter");
    let mut terminal = Terminal::new(PlatformConsole::new());
    #[cfg(target_os = "trueos")]
    let runtime = {
        let mut builder = trueos::runtime::current_thread();
        builder.enable_io();
        builder.build()
    };
    #[cfg(not(target_os = "trueos"))]
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(_) => {
            let _ = terminal.write(b"ssh: runtime initialization failed\r\n");
            return;
        }
    };
    #[cfg(target_os = "trueos")]
    let lease = match trueos::vshell::terminal_initial_lease() {
        Ok(lease) => lease,
        Err(_) => {
            let _ = terminal.write(b"ssh: cannot acquire terminal\r\n");
            return;
        }
    };
    #[cfg(target_os = "trueos")]
    {
        let (cols, rows) = terminal.size();
        let _ = trueos::vshell::konsole_begin_frame(
            cols,
            rows,
            trueos::vshell::KONSOLE_FRAME_TERMINAL_HANDOFF,
        );
    }
    let result = runtime.block_on(async {
        let args: Vec<_> = process_args().into_iter().skip(1).collect();
        let initial = if args.is_empty() {
            None
        } else {
            match Options::parse_args(args) {
                Ok(Some(options)) => Some(options),
                Ok(None) => {
                    terminal.write_all(options::USAGE.as_bytes(), None).await?;
                    None
                }
                Err(error) => {
                    terminal
                        .write_all(format!("{error}\r\n{}", options::USAGE).as_bytes(), None)
                        .await?;
                    None
                }
            }
        };
        terminal
            .write_all(
                b"ssh: resident native minishell; Ctrl-C or ~. returns after a session\r\n",
                None,
            )
            .await?;
        #[cfg(target_os = "trueos")]
        lease
            .acknowledge_ready()
            .map_err(|_| String::from("ssh: terminal readiness failed"))?;
        stage_log("runtime-ready");
        minishell(initial, &mut terminal).await
    });
    if let Err(error) = result {
        let _ = terminal.write(format!("\r\nssh: {error}\r\n").as_bytes());
    }
    runtime.shutdown_background();
    #[cfg(target_os = "trueos")]
    {
        stage_log("minishell-ended");
        let _ = lease.release_to_shell();
        let _ = trueos::vshell::shutdown_current_blueprint("ssh minishell exited");
    }
}
async fn minishell<C: terminal::Console>(
    mut next: Option<Options>,
    terminal: &mut Terminal<C>,
) -> Result<()> {
    let mut store = NativeStore;
    loop {
        let options = match next.take() {
            Some(options) => options,
            None => loop {
                let line = match terminal.line("ssh> ", true, None).await {
                    Ok(line) => line,
                    Err(error) => {
                        terminal
                            .write_all(format!("\r\n{error}\r\n").as_bytes(), None)
                            .await?;
                        continue;
                    }
                };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if matches!(line, "exit" | "quit") {
                    return Ok(());
                }
                if matches!(line, "help" | "?") {
                    terminal.write_all(b"ssh [options] [user@]host[:port]; Ctrl-C or ~. returns here; exit leaves the VM\r\n",None).await?;
                    continue;
                }
                match Options::parse_args(line.split_whitespace().map(String::from)) {
                    Ok(Some(options)) => break options,
                    Ok(None) => terminal.write_all(options::USAGE.as_bytes(), None).await?,
                    Err(error) => {
                        terminal
                            .write_all(format!("{error}\r\n{}", options::USAGE).as_bytes(), None)
                            .await?
                    }
                }
            },
        };
        if let Err(error) = session::connect(&options, &mut store, terminal).await {
            terminal
                .write_all(format!("\r\nssh: {error}\r\n").as_bytes(), None)
                .await?;
        }
        terminal
            .write_all(
                b"ssh: session returned; another destination or `exit`\r\n",
                None,
            )
            .await?;
    }
}
