//! Small terminal companion: output, one URL prompt, and an explicit script run.
use std::{
    collections::VecDeque,
    io::{self, Write},
    time::Duration,
};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute, queue,
    style::{Print, ResetColor},
    terminal::{
        self, Clear, ClearType, DisableLineWrap, EnableLineWrap, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use trueos::vshell::{self, TerminalLease, TerminalParkingTicket, TerminalReentry};

use crate::shellio;

pub(super) enum Command {
    Url(String),
    Run,
    Scripts,
    Script(String),
    Exit,
}

pub(super) struct Console {
    lease: Option<TerminalLease>,
    parked: Option<TerminalParkingTicket>,
    raw: bool,
    history: VecDeque<String>,
    input: String,
    input_overflow: bool,
    script: Option<shellio::ScriptInput>,
    dirty: bool,
}

impl Console {
    pub fn start() -> io::Result<Self> {
        let mut console = Self {
            lease: Some(vshell::terminal_initial_lease().map_err(lease_error)?),
            parked: None,
            raw: false,
            history: VecDeque::new(),
            input: String::new(),
            input_overflow: false,
            script: None,
            dirty: true,
        };
        console.activate()?;
        Ok(console)
    }

    fn activate(&mut self) -> io::Result<()> {
        terminal::enable_raw_mode()?;
        self.raw = true;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            DisableLineWrap,
            EnableBracketedPaste,
            Hide
        )?;
        #[cfg(unix)]
        event::discard_buffered_input()?;
        self.dirty = true;
        self.draw()?;
        self.lease
            .as_ref()
            .expect("active console lease")
            .acknowledge_ready()
            .map_err(lease_error)
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.raw {
            return Ok(());
        }
        let screen = execute!(
            io::stdout(),
            ResetColor,
            Show,
            DisableBracketedPaste,
            EnableLineWrap,
            LeaveAlternateScreen
        );
        let raw = terminal::disable_raw_mode();
        self.raw = false;
        screen.and(raw)
    }

    fn park(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        let _ = event::discard_buffered_input();
        let restored = self.restore();
        if let Some(lease) = self.lease.take() {
            self.parked = Some(lease.release_to_shell().map_err(lease_error)?);
        }
        self.input.clear();
        self.input_overflow = false;
        self.script = None;
        restored
    }

    /// Poll terminal RX only; Winit keeps ownership of browser-window events.
    pub fn poll(&mut self) -> io::Result<Vec<Command>> {
        for line in shellio::take_output() {
            if self.history.len() == 256 {
                self.history.pop_front();
            }
            self.history.push_back(line);
            self.dirty = true;
        }
        if let Some(ticket) = &self.parked {
            match ticket.poll_reentry().map_err(lease_error)? {
                TerminalReentry::Pending => return Ok(Vec::new()),
                TerminalReentry::Ready(lease) => {
                    self.parked = None;
                    self.lease = Some(lease);
                    self.activate()?;
                }
            }
        }
        let mut commands = Vec::new();
        for _ in 0..32 {
            if !event::poll(Duration::ZERO)? {
                break;
            }
            match event::read()? {
                Event::Resize(_, _) => self.dirty = true,
                Event::Paste(text) => {
                    if let Some(script) = &mut self.script {
                        script.append(&std::mem::take(&mut self.input));
                        script.append(&text);
                    } else if text.contains('\n') || text.contains('\r') {
                        shellio::line("hull: use :paste before pasting a multiline script");
                    } else if self.input.len().saturating_add(text.len()) <= 4096 {
                        self.input
                            .extend(text.chars().filter(|character| !character.is_control()));
                    } else {
                        self.input_overflow = true;
                    }
                    self.dirty = true;
                }
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        match key.code {
                            KeyCode::Char('q' | 'Q') => {
                                commands.push(Command::Exit);
                                break;
                            }
                            KeyCode::Char('c' | 'C') => {
                                self.input.clear();
                                self.input_overflow = false;
                                self.script = None;
                            }
                            KeyCode::Char('u' | 'U') => {
                                self.input.clear();
                                self.input_overflow = false;
                            }
                            _ => continue,
                        }
                    } else {
                        match key.code {
                            KeyCode::Esc => {
                                self.park()?;
                                break;
                            }
                            KeyCode::Backspace => {
                                self.input.pop();
                            }
                            KeyCode::Enter => {
                                self.submit(&mut commands);
                            }
                            KeyCode::Char(character) if !character.is_control() => {
                                let limit = if self.script.is_some() {
                                    shellio::MAX_SCRIPT_BYTES
                                } else {
                                    4096
                                };
                                if self.input.len() + character.len_utf8() <= limit
                                    && !self.input_overflow
                                {
                                    self.input.push(character);
                                } else {
                                    self.input_overflow = true;
                                }
                            }
                            _ => continue,
                        }
                    }
                    self.dirty = true;
                }
                _ => {}
            }
        }
        if self.raw && self.dirty {
            self.draw()?;
        }
        Ok(commands)
    }

    fn submit(&mut self, commands: &mut Vec<Command>) {
        let input = std::mem::take(&mut self.input);
        let overflow = std::mem::take(&mut self.input_overflow);
        if self.script.is_some() {
            if overflow {
                self.script = None;
                shellio::line("hull: script exceeds 64 KiB; paste was discarded");
            } else if input.trim() == ":cancel" {
                self.script = None;
            } else if input.trim() == ":end" {
                match self.script.take().expect("paste mode").finish() {
                    Ok(source) => commands.push(Command::Script(source)),
                    Err(error) => shellio::linef(format_args!("hull: {error}")),
                }
            } else {
                let script = self.script.as_mut().expect("paste mode");
                script.append(&input);
                script.append("\n");
            }
        } else if overflow {
            shellio::line("hull: URL is too long");
        } else {
            match input.trim() {
                "" => {}
                ":run" => commands.push(Command::Run),
                ":scripts" => commands.push(Command::Scripts),
                ":paste" => self.script = Some(shellio::ScriptInput::default()),
                input => match shellio::page_url(input) {
                    Ok(url) => commands.push(Command::Url(url)),
                    Err(error) => shellio::linef(format_args!("hull: {error}")),
                },
            }
        }
    }

    fn draw(&mut self) -> io::Result<()> {
        let (columns, rows) = terminal::size()?;
        if columns == 0 || rows == 0 {
            return Ok(());
        }
        let mut output = io::stdout();
        queue!(
            output,
            Hide,
            ResetColor,
            MoveTo(0, 0),
            Clear(ClearType::All)
        )?;
        let heading = if let Some(script) = &self.script {
            format!(
                "Hull console | script {} bytes | :end executes | :cancel discards",
                script.len() + self.input.len()
            )
        } else {
            "Hull | HTTPS URL | :scripts :run :paste | Esc shell | Ctrl-Q exit".to_owned()
        };
        let header: String = heading.chars().take(columns as usize).collect();
        queue!(output, Print(header))?;
        let count = rows.saturating_sub(2) as usize;
        for (row, line) in self
            .history
            .iter()
            .skip(self.history.len().saturating_sub(count))
            .enumerate()
        {
            let clipped: String = line.chars().take(columns as usize).collect();
            queue!(output, MoveTo(0, row as u16 + 1), Print(clipped))?;
        }
        let prefix = if self.script.is_some() { "js> " } else { "> " };
        let prefix: String = prefix.chars().take(columns as usize).collect();
        let prefix_width = prefix.len();
        let available = (columns as usize).saturating_sub(prefix_width);
        let prompt: String = self
            .input
            .chars()
            .rev()
            .take(available)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        queue!(
            output,
            MoveTo(0, rows - 1),
            Print(prefix),
            Print(&prompt),
            MoveTo(
                (prefix_width + prompt.chars().count()).min(columns.saturating_sub(1) as usize)
                    as u16,
                rows - 1
            ),
            Show
        )?;
        output.flush()?;
        self.dirty = false;
        Ok(())
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.restore();
        if let Some(lease) = self.lease.take() {
            let _ = lease.release_to_shell();
        }
    }
}

fn lease_error(error: vshell::TerminalLeaseError) -> io::Error {
    io::Error::other(error.to_string())
}
