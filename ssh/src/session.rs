//! One IO-less Sunset client with bounded, retained native byte queues.
use crate::{
    Result,
    escape::LocalEscape,
    options::{KNOWN_HOSTS, Options},
    queue::Queue,
    storage::{self, KnownHostStatus, Store},
    terminal::{Console, Terminal, check_deadline},
    wire::{TcpWire, Wire},
};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::time::Duration;
use sunset::{ChanData, ChanHandle, CliEvent, CliSessionExit, Client, Event, Runner};
use tokio::time::Instant;
use zeroize::Zeroizing;
const SETUP: Duration = Duration::from_secs(15);
const IDLE: Duration = Duration::from_secs(600);
const CLEANUP: Duration = Duration::from_secs(1);
const WORK: usize = 32;

struct State {
    rx: Queue,
    local: Queue,
    output: Queue,
    channel: Option<ChanHandle>,
    authenticated: bool,
    opened: bool,
    pty_sent: bool,
    shell_sent: bool,
    sent_size: (u32, u32),
    eof: bool,
    input_closed: bool,
    defunct: bool,
    exit: Option<String>,
    key_offered: bool,
    last_activity: Instant,
}
impl State {
    fn new() -> Result<Self> {
        Ok(Self {
            rx: Queue::new(32768)?,
            local: Queue::new(65536)?,
            output: Queue::new(32768)?,
            channel: None,
            authenticated: false,
            opened: false,
            pty_sent: false,
            shell_sent: false,
            sent_size: (0, 0),
            eof: false,
            input_closed: false,
            defunct: false,
            exit: None,
            key_offered: false,
            last_activity: Instant::now(),
        })
    }
}
fn engine(error: sunset::Error) -> String {
    format!("ssh: {error}")
}
// Used only while the caller still owns the exact request/bytes. A consumed
// authentication-response guard must surface its error instead of retrying.
fn retry_owned_send(error: &sunset::Error) -> bool {
    matches!(
        error,
        sunset::Error::NoRoom { .. } | sunset::Error::BusySend { .. }
    )
}

fn retry_channel_data(error: &sunset::Error) -> bool {
    matches!(
        error,
        sunset::Error::NoRoom { .. }
            | sunset::Error::BusySend {
                unsupported: false,
                ..
            }
    )
}

pub async fn connect<S: Store, C: Console>(
    options: &Options,
    store: &mut S,
    terminal: &mut Terminal<C>,
) -> Result<()> {
    let deadline = Instant::now() + SETUP;
    let mut wire = TcpWire::connect(&options.host, options.port, deadline).await?;
    run(&mut wire, options, store, terminal, deadline).await
}
/// Generic IO edge lets host fixtures inject short writes and backpressure while
/// using exactly the same protocol and terminal state machine as native sessions.
pub async fn run<W: Wire, S: Store, C: Console>(
    wire: &mut W,
    options: &Options,
    store: &mut S,
    terminal: &mut Terminal<C>,
    setup_deadline: Instant,
) -> Result<()> {
    let mut runner = Runner::new_client_owned();
    let mut state = State::new()?;
    let result = drive(
        wire,
        &mut runner,
        &mut state,
        options,
        store,
        terminal,
        setup_deadline,
    )
    .await;
    // A failed unanswered auth guard cannot be progressed again. Disconnect is
    // still allowed; accepted ciphertext and terminal output remain drainable.
    let _ = close(wire, &mut runner, &mut state, terminal, result.is_ok()).await;
    if let Some(exit) = state.exit.as_ref() {
        let _ = terminal
            .write_all(
                format!("\r\nssh: {exit}\r\n").as_bytes(),
                Some(Instant::now() + CLEANUP),
            )
            .await;
    }
    result
}

async fn drive<W: Wire, S: Store, C: Console>(
    wire: &mut W,
    runner: &mut Runner<'_, Client>,
    state: &mut State,
    options: &Options,
    store: &mut S,
    terminal: &mut Terminal<C>,
    setup_deadline: Instant,
) -> Result<()> {
    let mut escape = LocalEscape::new();
    loop {
        let setup = if runner.initial_kex_complete() {
            None
        } else {
            Some(setup_deadline)
        };
        check_deadline(setup)?;
        check_deadline(Some(state.last_activity + IDLE))?;
        let mut progress = false;
        for _ in 0..WORK {
            progress |= io_step(wire, runner, state, terminal)?;
            if runner.read_channel_ready().is_some() || state.defunct {
                break;
            } // Terminal must accept its retained prefix first.
            let prompt_deadline = if runner.initial_kex_complete() {
                Some(state.last_activity + IDLE)
            } else {
                Some(setup_deadline)
            };
            let mut became_authenticated = false;
            match runner.progress().map_err(engine)? {
                Event::None => {}
                Event::Progressed => progress = true,
                Event::Serv(_) => return Err("ssh: unexpected server-side event".into()),
                Event::Cli(event) => {
                    progress = true;
                    match event {
                        CliEvent::Hostkey(event) => {
                            let key = event.hostkey().map_err(engine)?;
                            let accepted =
                                trust(store, terminal, options, &key, prompt_deadline).await?;
                            if accepted {
                                event.accept().map_err(engine)?
                            } else {
                                event.reject().map_err(engine)?;
                                return Err("ssh: server host key refused".into());
                            }
                        }
                        CliEvent::Username(event) => {
                            event.username(&options.user).map_err(engine)?
                        }
                        CliEvent::Pubkey(event) => {
                            if state.key_offered {
                                event.skip().map_err(engine)?
                            } else {
                                state.key_offered = true;
                                let identity =
                                    storage::load_or_create_identity(store, &options.identity)
                                        .await?;
                                if identity.created {
                                    let public =
                                        identity.key.pubkey().to_openssh().map_err(engine)?;
                                    terminal.write_all(format!("ssh: generated Ed25519 identity {}\r\nssh: public key: {public}\r\n",options.identity).as_bytes(),prompt_deadline).await?;
                                }
                                event.pubkey(identity.key).map_err(engine)?;
                            }
                        }
                        CliEvent::KeyboardInteractive(event) => {
                            let info = event.info().map_err(engine)?;
                            let mut context = String::new();
                            for label in [info.name, info.instruction] {
                                let text = label.to_str().map_err(engine)?;
                                if !text.is_empty() {
                                    context.push_str(text);
                                    context.push_str("\r\n");
                                }
                            }
                            if info.prompts.is_empty() {
                                terminal.write_all(context.as_bytes(), prompt_deadline).await?;
                            }
                            let mut answers = Vec::<Zeroizing<String>>::new();
                            answers
                                .try_reserve_exact(info.prompts.len())
                                .map_err(|_| "ssh: cannot allocate authentication answers")?;
                            for prompt in &info.prompts {
                                answers.push(
                                    terminal
                                        .line(
                                            &format!("{context}{}", prompt.prompt.to_str().map_err(engine)?),
                                            prompt.echo,
                                            prompt_deadline,
                                        )
                                        .await?,
                                )
                            }
                            let references: Vec<&str> =
                                answers.iter().map(|s| s.as_str()).collect();
                            event.respond(&references).map_err(engine)?;
                        }
                        CliEvent::Password(event) => {
                            let password = terminal
                                .line(
                                    &format!("{}@{} password: ", options.user, options.host),
                                    false,
                                    prompt_deadline,
                                )
                                .await?;
                            event.password(password.as_str()).map_err(engine)?
                        }
                        CliEvent::AgentSign(event) => event.skip().map_err(engine)?,
                        CliEvent::Banner(event) => {
                            terminal
                                .write_all(event.raw_banner().0, prompt_deadline)
                                .await?;
                            terminal.write_all(b"\r\n", prompt_deadline).await?;
                        }
                        CliEvent::Authenticated => {
                            state.authenticated = true;
                            became_authenticated = true;
                        }
                        CliEvent::SessionOpened(_) => state.opened = true,
                        CliEvent::SessionExit(exit) => {
                            state.exit = Some(match exit {
                                CliSessionExit::Status(code) => {
                                    format!("remote exit status {code}")
                                }
                                CliSessionExit::Signal(signal) => {
                                    format!("remote exit signal {}", signal.signal)
                                }
                            })
                        }
                        CliEvent::Defunct => state.defunct = true,
                        CliEvent::PollAgain => {}
                    }
                }
            }
            if became_authenticated {
                crate::stage_log("authenticated");
            }
            if state.authenticated && state.channel.is_none() {
                match runner.open_client_session() {
                    Ok(channel) => state.channel = Some(channel),
                    Err(e) if retry_owned_send(&e) => {}
                    Err(e) => return Err(engine(e)),
                }
            }
            if state.opened {
                let channel = state
                    .channel
                    .as_ref()
                    .ok_or("ssh: missing session channel")?;
                if !state.pty_sent {
                    let (cols, rows) = terminal.size();
                    let term =
                        crate::process_var("TERM").unwrap_or_else(|| "xterm-256color".into());
                    let mut pty = sunset::Pty {
                        term: term
                            .as_str()
                            .try_into()
                            .map_err(|_| "ssh: TERM name is too long")?,
                        cols,
                        rows,
                        width: 0,
                        height: 0,
                        modes: Default::default(),
                    };
                    pty.modes
                        .push(sunset::ModePair { opcode: 53, arg: 1 })
                        .map_err(|_| "ssh: cannot encode PTY echo mode")?;
                    match runner.request_pty(channel, pty) {
                        Ok(()) => {
                            state.pty_sent = true;
                            state.sent_size = (cols, rows)
                        }
                        Err(e) if retry_owned_send(&e) => {}
                        Err(e) => return Err(engine(e)),
                    }
                }
                if state.pty_sent && !state.shell_sent {
                    match runner.request_shell(channel) {
                        Ok(()) => {
                            state.shell_sent = true;
                            terminal.write_all(b"ssh: authenticated; local escape is ~. at the start of a line\r\n",None).await?;
                        }
                        Err(e) if retry_owned_send(&e) => {}
                        Err(e) => return Err(engine(e)),
                    }
                }
            }
            // ExitStatus is metadata, not a byte-stream boundary. Servers may
            // still send normal/stderr data after it. Drain through remote EOF
            // or close, then wait for every retained terminal byte to be accepted.
            if remote_finished(runner, state) {
                return Ok(());
            }
            if !runner.is_input_ready() && !runner.is_output_pending() {
                break;
            }
        }
        if remote_finished(runner, state) {
            return Ok(());
        }
        let remote_ended = state.defunct
            || state
                .channel
                .as_ref()
                .is_some_and(|channel| runner.is_channel_eof(channel));
        if state.shell_sent && !remote_ended {
            let channel = state
                .channel
                .as_ref()
                .ok_or("ssh: missing session channel")?;
            let mut bytes = [0; 4096];
            let cap = state.local.room().saturating_sub(1).min(bytes.len());
            if cap != 0 {
                let n = terminal.read(&mut bytes[..cap])?;
                if n != 0 {
                    if bytes[..n].contains(&3) {
                        terminal
                            .write_all(b"\r\nssh: interrupted\r\n", None)
                            .await?;
                        return Ok(());
                    }
                    let mut forwarded = Vec::new();
                    forwarded
                        .try_reserve(n + 1)
                        .map_err(|_| "ssh: cannot allocate local input")?;
                    if escape.forward(&bytes[..n], &mut forwarded) {
                        terminal
                            .write_all(b"\r\nssh: disconnected by local escape\r\n", None)
                            .await?;
                        return Ok(());
                    }
                    state.local.append(&forwarded)?;
                }
            }
            if !state.local.is_empty() {
                let n = match runner.write_channel(channel, ChanData::Normal, state.local.bytes()) {
                    Ok(n) => n,
                    Err(e) if retry_channel_data(&e) => 0,
                    Err(sunset::Error::ChannelEOF) => return Ok(()),
                    Err(e) => return Err(engine(e)),
                };
                state.local.consume(n);
                if n != 0 {
                    state.last_activity = Instant::now();
                    progress = true
                }
            }
            let (cols, rows) = terminal.size();
            if (cols, rows) != state.sent_size {
                let size = sunset::packets::WinChange {
                    cols,
                    rows,
                    width: 0,
                    height: 0,
                };
                match runner.term_window_change(channel, &size) {
                    Ok(()) => {
                        state.sent_size = (cols, rows);
                        progress = true;
                        state.last_activity = Instant::now()
                    }
                    Err(e) if retry_owned_send(&e) => {}
                    Err(e) => return Err(engine(e)),
                }
            }
        }
        if progress {
            tokio::task::yield_now().await
        } else {
            wire.wait(
                !state.eof && state.rx.room() != 0 && runner.is_input_ready(),
                runner.is_output_pending(),
            )
            .await?
        }
    }
}

fn remote_finished(runner: &Runner<'_, Client>, state: &State) -> bool {
    let boundary = state.defunct
        || state.channel.as_ref().is_some_and(|channel| {
            runner.is_channel_closed(channel)
                || (state.exit.is_some() && runner.is_channel_eof(channel))
        });
    boundary
        && state.output.is_empty()
        && state.rx.is_empty()
        && runner.read_channel_ready().is_none()
}

fn io_step<W: Wire, C: Console>(
    wire: &mut W,
    runner: &mut Runner<'_, Client>,
    state: &mut State,
    terminal: &mut Terminal<C>,
) -> Result<bool> {
    let mut moved = false;
    if runner.is_output_pending() {
        let bytes = runner.output_buf();
        if let Some(n) = wire.write(bytes)? {
            if n > bytes.len() {
                return Err("ssh: invalid TCP write count".into());
            }
            runner.consume_output(n);
            moved |= n != 0;
            if n != 0 {
                state.last_activity = Instant::now()
            }
        }
    }
    if !state.output.is_empty() {
        let n = terminal.write(state.output.bytes())?;
        state.output.consume(n);
        moved |= n != 0;
    }
    if let Some(channel) = state.channel.as_ref() {
        if state.output.room() != 0 && runner.read_channel_ready().is_some() {
            let (n, _) = runner
                .read_channel_either(channel, state.output.spare())
                .map_err(engine)?;
            state.output.produced(n);
            moved |= n != 0;
            if n != 0 {
                state.last_activity = Instant::now()
            }
        }
    }
    if !state.rx.is_empty() && runner.is_input_ready() {
        let n = runner.input(state.rx.bytes()).map_err(engine)?;
        state.rx.consume(n);
        moved |= n != 0;
    }
    if !state.eof && state.rx.room() != 0 && runner.is_input_ready() {
        if let Some(n) = wire.read(state.rx.spare())? {
            if n == 0 {
                state.eof = true
            } else {
                state.rx.produced(n);
                state.last_activity = Instant::now();
                moved = true
            }
        }
    }
    // Deliver retained bytes before EOF. read_channel_either continues draining
    // decoded normal/stderr bytes even when the TCP input has closed.
    if state.eof && state.rx.is_empty() && !state.input_closed {
        runner.close_input();
        state.input_closed = true;
        moved = true
    }
    Ok(moved)
}

async fn trust<S: Store, C: Console>(
    store: &mut S,
    terminal: &mut Terminal<C>,
    options: &Options,
    key: &sunset::PubKey<'_>,
    deadline: Option<Instant>,
) -> Result<bool> {
    let snapshot = store
        .snapshot(KNOWN_HOSTS)
        .await
        .map_err(|e| e.to_string())?;
    match storage::known_host_status(snapshot.as_ref(), &options.host_token, key)? {
        KnownHostStatus::Match => Ok(true),
        KnownHostStatus::Changed => {
            terminal
                .write_all(
                    format!(
                        "\r\nssh: HOST KEY CHANGED for {}; refusing connection\r\n",
                        options.host_token
                    )
                    .as_bytes(),
                    deadline,
                )
                .await?;
            Ok(false)
        }
        KnownHostStatus::Unknown => {
            let fingerprint = key.fingerprint_sha256().map_err(engine)?;
            let context = format!("\r\nThe authenticity of host {} is unknown.\r\nFingerprint is {fingerprint}.\r\n", options.host_token);
            if options.accept_new {
                terminal.write_all(context.as_bytes(), deadline).await?;
            } else if !terminal
                    .line(&format!("{context}Trust this host key? [yes/no] "), true, deadline)
                    .await?
                    .eq_ignore_ascii_case("yes")
            {
                return Ok(false);
            }
            storage::learn_known_host(store, &options.host_token, key).await?;
            Ok(true)
        }
    }
}

async fn close<W: Wire, C: Console>(
    wire: &mut W,
    runner: &mut Runner<'_, Client>,
    state: &mut State,
    terminal: &mut Terminal<C>,
    may_progress: bool,
) -> Result<()> {
    let deadline = Instant::now() + CLEANUP;
    let (mut sent_eof, mut sent_close, mut disconnected) = (false, false, false);
    loop {
        if Instant::now() >= deadline {
            break;
        }
        let _ = io_step(wire, runner, state, terminal);
        if let Some(channel) = state.channel.as_ref() {
            if !sent_eof {
                match runner.send_channel_eof(channel) {
                    Ok(()) => sent_eof = true,
                    Err(e) if retry_owned_send(&e) => {}
                    Err(_) => sent_eof = true,
                }
            }
            if sent_eof && !sent_close {
                match runner.close_channel(channel) {
                    Ok(()) => sent_close = true,
                    Err(e) if retry_owned_send(&e) => {}
                    Err(_) => sent_close = true,
                }
            }
        } else {
            sent_close = true
        }
        if sent_close && !disconnected {
            match runner.disconnect() {
                Ok(()) => disconnected = true,
                Err(e) if retry_owned_send(&e) => {}
                Err(_) => disconnected = true,
            }
        }
        if may_progress && runner.read_channel_ready().is_none() {
            match runner.progress() {
                Ok(Event::Cli(CliEvent::SessionExit(exit))) => {
                    state.exit = Some(match exit {
                        CliSessionExit::Status(code) => format!("remote exit status {code}"),
                        CliSessionExit::Signal(_) => "remote exited by signal".into(),
                    })
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
        let peer_closed = state.eof
            || state
                .channel
                .as_ref()
                .is_some_and(|c| runner.is_channel_closed(c));
        if disconnected
            && !runner.is_output_pending()
            && state.output.is_empty()
            && state.rx.is_empty()
            && runner.read_channel_ready().is_none()
            && peer_closed
        {
            break;
        }
        wire.wait(
            !state.eof && state.rx.room() != 0 && runner.is_input_ready(),
            runner.is_output_pending(),
        )
        .await?;
    }
    let _ = wire.shutdown().await;
    Ok(())
}
