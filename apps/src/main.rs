#![no_std]
extern crate alloc;
use alloc::{format, string::{String, ToString}, vec::Vec};
mod catalog;
mod pullbot;
mod script;
mod mouse;
mod fetch;
mod download;
mod bundle;
mod view;
use view::{View, visible};
use crossterm::io::{self, Write};
use core::time::Duration;
use crossterm::{execute, cursor::{Hide, Show}, style::{Color, ResetColor}, terminal::{self, EnterAlternateScreen, LeaveAlternateScreen, DisableLineWrap, EnableLineWrap}, event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, EnableMouseCapture, DisableMouseCapture, MouseButton, MouseEventKind}};
use trueos::{runtime, task::LocalSet};
const CATALOG: &str = "https://trueos.eu/apps";
// Match Shell3's three control rows; the content inherits the terminal background.
const CONTROL_BACKGROUND: Color = Color::Rgb {
    r: trueos::terminal_style::CONTROL_BACKGROUND_RGB[0],
    g: trueos::terminal_style::CONTROL_BACKGROUND_RGB[1],
    b: trueos::terminal_style::CONTROL_BACKGROUND_RGB[2],
};
struct Terminal;
impl Terminal {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let terminal = Self;
        execute!(io::stdout(), EnterAlternateScreen, DisableLineWrap, Hide, EnableMouseCapture)?;
        Ok(terminal)
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture, ResetColor, Show, EnableLineWrap, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}
async fn run() -> Result<Option<String>, String> {
    let mut view = View::default();
    let mut apps = Vec::new();
    let mut query = String::new();
    let mut selected = 0;
    view.draw(&apps, &query, selected, "Loading catalog…").map_err(|e|e.to_string())?;
    loop {
        match fetch::bytes(CATALOG, fetch::CATALOG_LIMIT).await {
            Ok(html) => { apps = catalog::parse(core::str::from_utf8(&html).map_err(|e| e.to_string())?); break; }
            Err(error) => {
                view.draw(&apps, &query, selected, &format!("{error} · R retry / Esc quit")).map_err(|e|e.to_string())?;
                loop {
                    if event::poll(Duration::from_millis(0)).map_err(|e|e.to_string())? {
                        if let Event::Key(key) = event::read().map_err(|e|e.to_string())? {
                            if key.kind == KeyEventKind::Release { continue; }
                            match key.code { KeyCode::Esc => return Ok(None), KeyCode::Char('r' | 'R') => break, _ => {} }
                        }
                    }
                    trueos::time::sleep(Duration::from_millis(25)).await;
                }
            }
        }
    }
    let mut message = String::from("Click an app to download and launch; Space adds it to AppDB without starting.");
    let mut dirty = true;
    loop {
        if dirty {
            view.draw(&apps, &query, selected, &message).map_err(|e|e.to_string())?;
            dirty = false;
        }
        // Batch input before rendering, but yield regularly under mouse traffic.
        let mut events = 0;
        while events < 64 && event::poll(Duration::from_millis(0)).map_err(|e|e.to_string())? {
            events += 1;
            let previous_selected = selected;
            let previous_query = query.clone();
            let ids = visible(&apps, &query);
            let mut launch = false;
            let mut save = false;
            match event::read().map_err(|e|e.to_string())? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') { return Ok(None); }
                    match key.code {
                        KeyCode::Esc => return Ok(None),
                        KeyCode::Up => selected = selected.saturating_sub(mouse::COLUMNS),
                        KeyCode::Down => selected = (selected + mouse::COLUMNS).min(ids.len().saturating_sub(1)),
                        KeyCode::Left => selected = selected.saturating_sub(1),
                        KeyCode::Right => selected = (selected + 1).min(ids.len().saturating_sub(1)),
                        KeyCode::Char(' ') => save = true,
                        KeyCode::PageUp => selected = selected.saturating_sub(mouse::page_size(terminal::size().map_err(|e|e.to_string())?.1)),
                        KeyCode::PageDown => selected = (selected + mouse::page_size(terminal::size().map_err(|e|e.to_string())?.1)).min(ids.len().saturating_sub(1)),
                        KeyCode::Backspace => { query.pop(); selected = 0; }
                        KeyCode::Char(ch) => { if query.len() < 128 { query.push(ch); selected = 0; } }
                        KeyCode::Enter => launch = true,
                        _ => {}
                    }
                }
                Event::Mouse(mouse_event) => match mouse_event.kind {
                    MouseEventKind::ScrollUp => { selected = selected.saturating_sub(1); }
                    MouseEventKind::ScrollDown => { selected = (selected + 1).min(ids.len().saturating_sub(1)); }
                    MouseEventKind::Moved | MouseEventKind::Down(MouseButton::Left) => {
                        let (width, height) = terminal::size().map_err(|e|e.to_string())?;
                        if let Some(position) = mouse::hit_test(width, height, selected, ids.len(), mouse_event.column, mouse_event.row) {
                            selected = position;
                            launch = matches!(mouse_event.kind, MouseEventKind::Down(MouseButton::Left));
                        }
                    }
                    _ => {}
                },
                Event::Resize(_, _) => dirty = true,
                _ => {}
            }
            dirty |= selected != previous_selected || query != previous_query;
            if launch || save {
                if let Some(id) = ids.get(selected) {
                    let app = &apps[*id];
                    view.draw(&apps,&query,selected,&format!("Downloading {}…",app.name)).map_err(|e|e.to_string())?;
                    dirty = true;
                    match bundle::download(&apps, app, save).await {
                        Ok(_) if save => message = format!("Added {} to AppDB; not started.", app.name),
                        Ok(path) => return Ok(Some(path)),
                        Err(error) => message = error,
                    }
                }
            }
        }
        trueos::time::sleep(Duration::from_millis(25)).await;
    }
}
fn main() {
    // Consume the one-shot launch stream before any terminal/UI initialization.
    if let Ok(script) = trueos::async_fs::block_on(trueos::async_fs::read_file(b"vFile:launch")) {
        match runtime::current_thread().build() {
            Ok(runtime) => {
                LocalSet::new().block_on(&runtime, pullbot::drain(&script));
                runtime.shutdown_background();
            }
            Err(error) => pullbot::report(&format!("runtime failed: {error}")),
        }
        let _ = trueos::vshell::shutdown_current_blueprint("apps pullbot drained");
        return;
    }
    let terminal = match Terminal::enter() { Ok(t) => t, Err(e) => { error_output(&format!("{e}")); return; } };
    let lease = trueos::vshell::terminal_initial_lease().ok();
    let runtime = match runtime::current_thread().build() { Ok(rt) => rt, Err(e) => { drop(terminal); error_output(&format!("{e}")); return; } };
    let result = LocalSet::new().block_on(&runtime, run());
    runtime.shutdown_background();
    drop(terminal);
    if let Some(lease) = lease { let _ = lease.release_to_shell(); }
    match result {
        Ok(Some(path)) => if let Err(code) = trueos::vshell::launch_with_script(&path, "") { error_output(&format!("launch failed ({code})")); },
        Ok(None) => {},
        Err(error) => error_output(&error),
    }
}

fn error_output(message: &str) {
    trueos::vsys::log_errorf(format_args!("apps: {message}\n"));
}
