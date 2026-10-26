use std::{
    io::{self, stdout, Stdout, Write},
    time::Duration,
};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, DisableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute, Print, ResetColor, SetAttribute},
    terminal::{
        self, Clear, ClearType, DisableLineWrap, EnableLineWrap, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};

const POLL: Duration = Duration::from_millis(40);

struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        if let Err(error) = execute!(stdout(), EnterAlternateScreen, DisableLineWrap, Hide) {
            let _ = execute!(
                stdout(),
                ResetColor,
                SetAttribute(Attribute::Reset),
                Show,
                EnableLineWrap,
                LeaveAlternateScreen
            );
            let _ = terminal::disable_raw_mode();
            return Err(error);
        }
        Ok(Self { active: true })
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }

        let screen = execute!(
            stdout(),
            ResetColor,
            SetAttribute(Attribute::Reset),
            Show,
            DisableMouseCapture,
            EnableLineWrap,
            LeaveAlternateScreen
        );
        let raw = terminal::disable_raw_mode();
        self.active = false;
        screen.and(raw)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionExit {
    Park,
    Shutdown,
}

fn lease_io(error: trueos::vshell::TerminalLeaseError) -> io::Error {
    io::Error::new(io::ErrorKind::Other, error.to_string())
}

pub fn run() -> io::Result<()> {
    let mut lease = trueos::vshell::terminal_initial_lease().map_err(lease_io)?;

    loop {
        let outcome = run_terminal_session(|| lease.acknowledge_ready().map_err(lease_io));
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                let reason = format!("termdir handoff probe failed: {error}");
                let _ = trueos::vshell::report_exit_reason(reason.as_str());
                let _ = lease.release_to_shell();
                return Err(error);
            }
        };

        match outcome {
            SessionExit::Shutdown => {
                let _ = trueos::vshell::report_exit_reason("termdir handoff probe user exit");
                let _ticket = lease.release_to_shell().map_err(lease_io)?;
                return Ok(());
            }
            SessionExit::Park => {
                let ticket = lease.release_to_shell().map_err(lease_io)?;
                lease = ticket.wait_for_reentry().map_err(lease_io)?;
            }
        }
    }
}

fn run_terminal_session(
    mut first_frame_ready: impl FnMut() -> io::Result<()>,
) -> io::Result<SessionExit> {
    let mut guard = TerminalGuard::enter()?;
    let mut out = stdout();

    let session = (|| {
        draw(&mut out)?;
        first_frame_ready()?;

        loop {
            if !event::poll(POLL)? {
                continue;
            }

            match event::read()? {
                Event::Resize(_, _) => draw(&mut out)?,
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('q' | 'Q'))
                    {
                        return Ok(SessionExit::Shutdown);
                    }

                    match key.code {
                        KeyCode::Esc => return Ok(SessionExit::Park),
                        KeyCode::Char('r' | 'R') => draw(&mut out)?,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    })();

    let restore = guard.restore();
    match session {
        Err(error) => {
            let _ = restore;
            Err(error)
        }
        Ok(outcome) => {
            restore?;
            Ok(outcome)
        }
    }
}

fn draw(out: &mut Stdout) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    queue!(
        out,
        ResetColor,
        SetAttribute(Attribute::Reset),
        MoveTo(0, 0),
        Clear(ClearType::All)
    )?;

    let lines = [
        "TRUEOS · termdir terminal handoff probe",
        "",
        "static demo target — filesystem/VFS modules are not compiled here",
        "",
        "        root",
        "       ╱    ╲",
        "    apps    system",
        "    ╱ ╲       ╲",
        " termdir tui  net",
        "",
        "Esc     restore Crossterm + release lease to Shell2",
        "tui     from Shell2 requests re-entry into this same process",
        "R       repaint current surface",
        "Ctrl-Q  restore terminal + release lease + exit",
        "",
        "Expected cycle: lease → first frame → ready ack → release → re-entry → repaint",
    ];

    let max_rows = rows.saturating_sub(1) as usize;
    for (row, line) in lines.iter().take(max_rows).enumerate() {
        let clipped: String = line.chars().take(cols as usize).collect();
        queue!(out, MoveTo(0, row as u16), Print(clipped))?;
    }

    out.flush()
}
