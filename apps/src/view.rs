//! Retain the painted list so selection changes touch only its two markers.
use alloc::{format, string::{String, ToString}, vec::Vec};
use crossterm::{
    cursor::MoveTo,
    io::{self, Write},
    queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
};
use super::{catalog::OnlineApp, mouse, CONTROL_BACKGROUND};

pub(crate) fn visible(apps: &[OnlineApp], query: &str) -> Vec<usize> {
    if let Ok(id) = query.parse::<usize>() {
        return (id < apps.len()).then_some(id).into_iter().collect();
    }
    let query = query.to_ascii_lowercase();
    apps.iter().enumerate().filter_map(|(id, app)| {
        app.name.to_ascii_lowercase().contains(&query).then_some(id)
    }).collect()
}

struct Painted {
    size: (u16, u16),
    count: usize,
    query: String,
    selected: usize,
    message: String,
}

#[derive(Default)]
pub(crate) struct View {
    painted: Option<Painted>,
}

impl View {
    pub(crate) fn draw(&mut self, apps: &[OnlineApp], query: &str, selected: usize, message: &str) -> io::Result<()> {
        self.draw_to(&mut io::stdout().lock(), terminal::size()?, apps, query, selected, message)
    }

    pub(crate) fn draw_to(&mut self, out: &mut impl Write, size: (u16, u16), apps: &[OnlineApp], query: &str, selected: usize, message: &str) -> io::Result<()> {
        let (width, height) = size;
        let rows = mouse::page_size(height);
        let ids = visible(apps, query);
        let start = selected / rows * rows;
        let full = self.painted.as_ref().is_none_or(|old| {
            old.size != size || old.count != apps.len() || old.query != query
                || old.selected / rows != selected / rows
        });
        let selection_changed = self.painted.as_ref().is_some_and(|old| old.selected != selected);
        let message_changed = self.painted.as_ref().is_none_or(|old| old.message != message);
        if !full && !selection_changed && !message_changed { return Ok(()); }

        // queue! writes immediately on TRUEOS stdout. Assemble the entire update
        // first; synchronized output also protects against transport chunking.
        let mut bytes = Vec::new();
        queue!(bytes, BeginSynchronizedUpdate, ResetColor)?;
        if full {
            queue!(bytes, Clear(ClearType::All), SetBackgroundColor(CONTROL_BACKGROUND),
                MoveTo(0,0), Clear(ClearType::CurrentLine),
                MoveTo(0,1), Clear(ClearType::CurrentLine),
                MoveTo(0,2), Clear(ClearType::CurrentLine),
                MoveTo(0,0), SetForegroundColor(Color::Cyan), Print("TRUEOS APPS"), SetForegroundColor(Color::Reset),
                MoveTo(0,1), Print(format!("{} apps · {} matches", apps.len(), ids.len())), ResetColor,
                MoveTo(0,3), Print(format!("Search / ID: {query}").chars().take(width as usize).collect::<String>()))?;
            for (offset, id) in ids.iter().skip(start).take(rows).enumerate() {
                let (x, y, cell_width) = mouse::cell(width, offset);
                let text = format!(" {:>2}  {}", id, apps[*id].name);
                queue!(bytes, MoveTo(x,y), Print(text.chars().take(cell_width).collect::<String>()))?;
            }
            queue!(bytes, MoveTo(0,height.saturating_sub(2)), SetForegroundColor(Color::DarkGrey),
                Print("Hover select · Click/Enter launch online · Space → AppDB only · Esc quit".chars().take(width as usize).collect::<String>()), ResetColor)?;
        } else if selection_changed {
            self.marker(&mut bytes, width, &ids, start, self.painted.as_ref().unwrap().selected, false)?;
        }
        if full || selection_changed {
            self.marker(&mut bytes, width, &ids, start, selected, true)?;
        }
        if full || message_changed {
            queue!(bytes, MoveTo(0,height.saturating_sub(1)), Clear(ClearType::CurrentLine),
                Print(message.chars().take(width as usize).collect::<String>()))?;
        }
        queue!(bytes, ResetColor, EndSynchronizedUpdate)?;
        out.write_all(&bytes)?;
        out.flush()?;
        self.painted = Some(Painted {
            size, count: apps.len(), query: query.to_string(), selected, message: message.to_string(),
        });
        Ok(())
    }

    fn marker(&self, out: &mut impl Write, width: u16, ids: &[usize], start: usize, selected: usize, enabled: bool) -> io::Result<()> {
        let Some(id) = ids.get(selected) else { return Ok(()); };
        let (x, y, cell_width) = mouse::cell(width, selected - start);
        let inset = 1 + id.to_string().len().max(2);
        let marker_width = cell_width.saturating_sub(inset).min(2);
        if marker_width == 0 { return Ok(()); }
        queue!(out, MoveTo(x + inset as u16, y), ResetColor)?;
        if enabled { queue!(out, SetBackgroundColor(Color::DarkCyan))?; }
        queue!(out, Print(" ".repeat(marker_width)), ResetColor)
    }
}
