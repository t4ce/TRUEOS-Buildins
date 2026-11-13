//! Retain the painted list so selection changes touch only its two markers.
use alloc::{collections::BTreeSet, format, string::{String, ToString}, vec::Vec};
use crossterm::{
    cursor::MoveTo,
    io::{self, Write},
    queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
};
use super::{catalog::OnlineApp, mouse, CONTROL_BACKGROUND};

pub(crate) fn visible(apps: &[OnlineApp], query: &str) -> Vec<usize> {
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
    hash: String,
}

#[derive(Default)]
pub(crate) struct View {
    painted: Option<Painted>,
    fetched: BTreeSet<String>,
}

impl View {
    pub(crate) fn set_fetched(&mut self, names: BTreeSet<String>) {
        if self.fetched != names {
            self.fetched = names;
            self.painted = None;
        }
    }

    pub(crate) fn draw(&mut self, apps: &[OnlineApp], query: &str, selected: usize, message: &str) -> io::Result<()> {
        self.draw_to(&mut io::stdout().lock(), terminal::size()?, apps, query, selected, message)
    }

    pub(crate) fn draw_to(&mut self, out: &mut impl Write, size: (u16, u16), apps: &[OnlineApp], query: &str, selected: usize, message: &str) -> io::Result<()> {
        let (width, height) = size;
        let rows = mouse::page_size(height);
        let ids = visible(apps, query);
        let start = selected / rows * rows;
        // Keep the centered hash clear of the title, including at small sizes.
        let hash_width = width.saturating_sub(2 * 13).min(33) as usize;
        let hash = middle_ellipsis(ids.get(selected).map_or("", |id| apps[*id].sha256.as_str()), hash_width);
        let full = self.painted.as_ref().is_none_or(|old| {
            old.size != size || old.count != apps.len() || old.query != query
                || old.selected / rows != selected / rows
        });
        let selection_changed = self.painted.as_ref().is_some_and(|old| old.selected != selected);
        let message_changed = self.painted.as_ref().is_none_or(|old| old.message != message);
        let hash_changed = self.painted.as_ref().is_none_or(|old| old.hash != hash);
        if !full && !selection_changed && !message_changed && !hash_changed { return Ok(()); }

        // queue! writes immediately on TRUEOS stdout. Assemble the entire update
        // first; synchronized output also protects against transport chunking.
        let mut bytes = Vec::new();
        queue!(bytes, BeginSynchronizedUpdate, ResetColor)?;
        if full {
            queue!(bytes, Clear(ClearType::All), SetBackgroundColor(CONTROL_BACKGROUND),
                MoveTo(0,0), Clear(ClearType::CurrentLine),
                MoveTo(0,1), Clear(ClearType::CurrentLine),
                MoveTo(0,2), Clear(ClearType::CurrentLine),
                MoveTo(0,0), SetForegroundColor(Color::Cyan), Print("TRUEOS APPS".chars().take(width as usize).collect::<String>()), SetForegroundColor(Color::Reset),
                MoveTo(0,1), Print(format!("{} apps · {} matches", apps.len(), ids.len())), ResetColor,
                MoveTo(0,3), Print(format!("Search: {query}").chars().take(width as usize).collect::<String>()))?;
            for (offset, id) in ids.iter().skip(start).take(rows).enumerate() {
                let (x, y, cell_width) = mouse::cell(width, offset);
                queue!(bytes, MoveTo(x,y), ResetColor, Print(" ".repeat(cell_width.min(2))))?;
                if self.fetched.contains(&apps[*id].archive_name) {
                    queue!(bytes, SetForegroundColor(Color::Grey), SetAttribute(Attribute::Bold))?;
                }
                queue!(bytes, Print(apps[*id].name.chars().take(cell_width.saturating_sub(2)).collect::<String>()), ResetColor)?;
            }
            queue!(bytes, MoveTo(0,height.saturating_sub(2)), SetForegroundColor(Color::Grey), SetAttribute(Attribute::Bold),
                Print("fetched".chars().take(width as usize).collect::<String>()), ResetColor, SetForegroundColor(Color::DarkGrey),
                Print(" · Hover select · Click/Enter launch · Space fetch · Esc quit".chars().take((width as usize).saturating_sub(7)).collect::<String>()), ResetColor)?;
        } else if selection_changed {
            self.marker(&mut bytes, width, &ids, start, self.painted.as_ref().unwrap().selected, false)?;
        }
        if full || selection_changed {
            self.marker(&mut bytes, width, &ids, start, selected, true)?;
        }
        if full || hash_changed {
            let left_pad = hash_width.saturating_sub(hash.chars().count()) / 2;
            let text = format!("{}{}{}", " ".repeat(left_pad), hash,
                " ".repeat(hash_width.saturating_sub(left_pad + hash.chars().count())));
            if hash_width > 0 {
                queue!(bytes, MoveTo((width - hash_width as u16) / 2, 0), SetBackgroundColor(CONTROL_BACKGROUND),
                    SetForegroundColor(Color::Grey), Print(text), ResetColor)?;
            }
        }
        if full || message_changed {
            queue!(bytes, MoveTo(0,height.saturating_sub(1)), Clear(ClearType::CurrentLine),
                Print(message.chars().take(width as usize).collect::<String>()))?;
        }
        queue!(bytes, ResetColor, EndSynchronizedUpdate)?;
        out.write_all(&bytes)?;
        out.flush()?;
        self.painted = Some(Painted {
            size, count: apps.len(), query: query.to_string(), selected, message: message.to_string(), hash,
        });
        Ok(())
    }

    fn marker(&self, out: &mut impl Write, width: u16, ids: &[usize], start: usize, selected: usize, enabled: bool) -> io::Result<()> {
        if ids.get(selected).is_none() { return Ok(()); }
        let (x, y, cell_width) = mouse::cell(width, selected - start);
        let marker_width = cell_width.min(2);
        if marker_width == 0 { return Ok(()); }
        queue!(out, MoveTo(x, y), ResetColor)?;
        if enabled { queue!(out, SetBackgroundColor(Color::DarkCyan))?; }
        queue!(out, Print(" ".repeat(marker_width)), ResetColor)
    }
}

fn middle_ellipsis(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len <= width { return text.into(); }
    if width == 0 { return String::new(); }
    let left = width / 2;
    let right = width - left - 1;
    format!("{}…{}", text.chars().take(left).collect::<String>(), text.chars().skip(len - right).collect::<String>())
}
