use crate::{
    actions::{
        MENU_ENTRIES, MenuCommand, MenuContext, MenuLink, MenuSection, MenuState, MountLink,
    },
    graph_view::{ContentTypeDisplay, LineStyle, SelectionStats},
    layout::LayoutMode,
    screen::{Frame, Style, text_cell_width},
};

const DIGIT_GLYPHS: [&str; 10] = ["🯰", "🯱", "🯲", "🯳", "🯴", "🯵", "🯶", "🯷", "🯸", "🯹"];
const TOGGLE_GLYPHS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

pub fn draw_menu(
    frame: &mut Frame,
    menu_x: u16,
    bottom: u16,
    menu: MenuState,
    menu_width: u16,
    context: MenuContext,
    mounts: &[MountLink],
    links: &[MenuLink],
    stats: Option<&SelectionStats>,
    zoom_step: usize,
    depth_limit: usize,
    spacing_level: usize,
    line_style: LineStyle,
    layout_mode: LayoutMode,
) {
    let _ = menu_width;
    let blank = "│               │";
    for y in 0..=bottom {
        menu_line(frame, menu_x, y, blank);
    }

    // One literal gap between the stylized E and N. The extra horizontal cap
    // keeps the close glyph at the physical top-right cell of the 17-col menu.
    menu_line(frame, menu_x, 0, "├─╼ ᗰ ☰ＮＵ╾─┤🯀");
    menu_line(frame, menu_x, 1, "│Use ⌨  Tab  ╰─╮");
    menu_line(frame, menu_x, 2, "├─╼ Mount      ╾┤");
    let mount_count = mounts.len().saturating_add(1);
    let visible_mounts = MenuState::visible_mount_count(mount_count);
    for index in 0..visible_mounts {
        let label = if index == 0 {
            "reload".to_string()
        } else {
            mounts[index - 1].menu_label()
        };
        let cursor = if menu.section == MenuSection::Mount && menu.mount_cursor == index {
            "☩"
        } else {
            " "
        };
        menu_line(
            frame,
            menu_x,
            3 + index as u16,
            &entry_line(index, cursor, &label),
        );
    }
    menu_line(frame, menu_x, 3 + visible_mounts as u16, blank);
    let view_header = MenuState::view_header_row(mount_count);
    let action_header = MenuState::action_header_row(mount_count);
    menu_line(frame, menu_x, view_header, "├─╼ View       ╾┤");
    menu_line(frame, menu_x, view_header + 7, "│               ╿");
    menu_line(frame, menu_x, action_header, "├─╼ Act        ╾┤");

    let view_rows = [
        (MenuCommand::Center, "center", None, None),
        (
            MenuCommand::Zoom,
            "zoom",
            Some(toggle_glyph(zoom_step.min(4), 5)),
            Some(('╭', '╮')),
        ),
        (
            MenuCommand::Depth,
            "depth",
            Some(TOGGLE_GLYPHS[depth_limit.min(7)]),
            Some(('┝', '┥')),
        ),
        (
            MenuCommand::Spacing,
            "space",
            Some(spacing_toggle_glyph(spacing_level.min(4))),
            Some(('┝', '┥')),
        ),
        (
            MenuCommand::LineStyle,
            "line",
            Some(toggle_glyph(line_style_index(line_style), 4)),
            Some(('┝', '┥')),
        ),
        (
            MenuCommand::ToggleLayout,
            "layout",
            Some(toggle_glyph(
                if layout_mode == LayoutMode::Tree {
                    0
                } else {
                    1
                },
                2,
            )),
            Some(('╰', '╯')),
        ),
    ];
    for (local, (command, label, toggle, cap)) in view_rows.iter().enumerate() {
        let Some(index) = MenuState::index_for_command(*command) else {
            continue;
        };
        let Some(y) = MenuState::row_for_index(index, context, mount_count) else {
            continue;
        };
        let cursor = if menu.section == MenuSection::View && menu.cursor == index {
            "☩"
        } else {
            " "
        };
        menu_line(
            frame,
            menu_x,
            y,
            &view_line(local, cursor, label, *toggle, *cap),
        );
    }

    for index in 0..MENU_ENTRIES.len() {
        let entry = MENU_ENTRIES[index];
        if entry.section != MenuSection::Action || !MenuState::is_visible(index, context) {
            continue;
        }
        let Some(y) = MenuState::row_for_index(index, context, mount_count) else {
            continue;
        };
        if y >= bottom {
            continue;
        }
        let local = MenuState::local_index(index, context).unwrap_or(0).min(9);
        let cursor = if menu.section == MenuSection::Action && menu.cursor == index {
            "☩"
        } else {
            " "
        };
        menu_line(frame, menu_x, y, &entry_line(local, cursor, entry.label));
    }

    let action_blank = action_header + 1 + MenuState::action_count(context) as u16;
    if action_blank < bottom {
        menu_line(frame, menu_x, action_blank, blank);
    }

    if let (Some(header), Some(stats)) = (MenuState::stats_header_row(context, mount_count), stats)
    {
        if header < bottom {
            menu_line(frame, menu_x, header, "├─╼ Stats      ╾┤");
            let rows = [
                ("key", stats.key.as_str()),
                ("size", stats.size.as_str()),
                ("kind", stats.kind.as_str()),
                ("mod", stats.modified.as_str()),
                ("mode", stats.access.as_str()),
            ];
            let available = bottom.saturating_sub(header + 1) as usize;
            for (offset, (key, value)) in rows.iter().enumerate().take(available) {
                menu_line(
                    frame,
                    menu_x,
                    header + 1 + offset as u16,
                    &stats_line(key, value),
                );
            }
            for (offset, (key, value)) in content_type_lines(&stats.content_type)
                .iter()
                .enumerate()
                .take(available.saturating_sub(rows.len()))
            {
                menu_line(
                    frame,
                    menu_x,
                    header + 6 + offset as u16,
                    &stats_line(key, value),
                );
            }
            if header + 16 < bottom {
                menu_line(frame, menu_x, header + 16, blank);
            }
        }
    }

    if let Some(clip_header) = MenuState::clip_header_row(bottom, context, mount_count, links.len())
    {
        menu_line(frame, menu_x, clip_header, "├─Clip 🖈       ╾┤");
        draw_links(frame, menu_x, bottom, context, mount_count, menu, links);
    }

    menu_line(frame, menu_x, bottom, "╰───────────────╯");
}

pub fn close_button_hit(menu_x: u16, menu_width: u16, x: u16, y: u16) -> bool {
    y == 0 && x == menu_x.saturating_add(menu_width.saturating_sub(1))
}

fn line_style_index(style: LineStyle) -> usize {
    match style {
        LineStyle::Default => 0,
        LineStyle::Straight => 1,
        LineStyle::Elbow => 2,
        LineStyle::SoftArc => 3,
    }
}

fn spacing_toggle_glyph(index: usize) -> &'static str {
    const SLOTS: [usize; 5] = [0, 1, 3, 5, 7];
    TOGGLE_GLYPHS[SLOTS[index.min(4)]]
}

fn toggle_glyph(index: usize, states: usize) -> &'static str {
    if states <= 1 {
        return TOGGLE_GLYPHS[0];
    }
    let index = index.min(states - 1);
    let slot = (index * 7 + (states - 1) / 2) / (states - 1);
    TOGGLE_GLYPHS[slot.min(7)]
}

fn entry_line(local: usize, cursor: &str, label: &str) -> String {
    let digit = DIGIT_GLYPHS[local.min(9)];
    let label = pad_cells(&clip_cells(label, 11), 11);
    format!("│{digit} {cursor} {label}│")
}

fn view_line(
    local: usize,
    cursor: &str,
    label: &str,
    toggle: Option<&str>,
    cap: Option<(char, char)>,
) -> String {
    let digit = DIGIT_GLYPHS[local.min(9)];
    let prefix = format!("│{digit} {cursor} ");
    let rail = match (toggle, cap) {
        (Some(toggle), Some((left, right))) => format!("{left}{toggle}{right}"),
        _ => "╽".to_string(),
    };
    let budget = 17usize
        .saturating_sub(text_cell_width(&prefix))
        .saturating_sub(text_cell_width(&rail));
    let label = clip_cells(label, budget);
    let gap = " ".repeat(budget.saturating_sub(text_cell_width(&label)));
    format!("{prefix}{label}{gap}{rail}")
}

fn stats_line(key: &str, value: &str) -> String {
    let key = pad_cells(&clip_cells(key, 5), 5);
    let value = pad_cells(&clip_cells(value, 10), 10);
    format!("│{key}{value}│")
}

/// Keep each durable identity component inspectable in the 15-column stats
/// value cell. The source string is produced from typed metadata; this only
/// lays it out and never derives anything from names or bytes.
fn content_type_lines(value: &ContentTypeDisplay) -> Vec<(String, String)> {
    let mut lines = Vec::new();
    append_wrapped_stats_value(&mut lines, "id", value.raw_hex.as_str());
    append_wrapped_stats_value(&mut lines, "raw", value.decimal.as_str());
    append_wrapped_stats_value(&mut lines, "name", value.canonical_name.as_str());
    append_wrapped_stats_value(&mut lines, "mime", value.mime.as_str());
    append_wrapped_stats_value(&mut lines, "state", value.status.as_str());
    lines
}

fn append_wrapped_stats_value(lines: &mut Vec<(String, String)>, key: &str, value: &str) {
    // Registry descriptors are ASCII. Keeping the split at the same ten-byte
    // boundary as `stats_line` makes every byte visible without ellipses.
    let mut chunks = value.as_bytes().chunks(10).peekable();
    if chunks.peek().is_none() {
        lines.push((key.to_string(), String::new()));
        return;
    }
    for (index, chunk) in chunks.enumerate() {
        lines.push((
            if index == 0 { key } else { "" }.to_string(),
            String::from_utf8_lossy(chunk).into_owned(),
        ));
    }
}

#[cfg(test)]
mod typed_stats_tests {
    use super::{ContentTypeDisplay, content_type_lines};

    #[test]
    fn lays_out_all_identity_fields_without_clipping() {
        assert_eq!(
            content_type_lines(&ContentTypeDisplay {
                raw_hex: "0x00000002".into(),
                decimal: "2".into(),
                canonical_name: "TTF_LONG_NAME".into(),
                mime: "application/font-sfnt".into(),
                status: "registered".into(),
            }),
            vec![
                ("id", "0x00000002"),
                ("raw", "2"),
                ("name", "TTF_LONG_N"),
                ("", "AME"),
                ("mime", "applicatio"),
                ("", "n/font-sfnt"),
                ("state", "registered")
            ]
            .into_iter()
            .map(|(key, value)| (String::from(key), String::from(value)))
            .collect::<Vec<_>>()
        );
    }
}

fn draw_links(
    frame: &mut Frame,
    menu_x: u16,
    bottom: u16,
    context: MenuContext,
    mount_count: usize,
    menu: MenuState,
    links: &[MenuLink],
) {
    let visible = MenuState::clip_visible_count(bottom, context, mount_count, links.len());
    if visible == 0 {
        return;
    }

    if links.is_empty() {
        let y = bottom - 1;
        let cursor = if menu.section == MenuSection::Clip {
            "☩"
        } else {
            " "
        };
        menu_line(frame, menu_x, y, &entry_line(0, cursor, "Empty"));
        return;
    }

    // The Clip section is bottom-anchored. First stays nearest the bottom;
    // subsequent links stack upward, and tight terminals simply show fewer.
    for index in 0..visible {
        let y = bottom - 1 - index as u16;
        let cursor = if menu.section == MenuSection::Clip && menu.clip_cursor == index {
            "☩"
        } else {
            " "
        };
        menu_line(
            frame,
            menu_x,
            y,
            &entry_line(index, cursor, &links[index].label()),
        );
    }
}

pub fn link_index_for_row(
    row: u16,
    bottom: u16,
    context: MenuContext,
    mount_count: usize,
    link_count: usize,
) -> Option<usize> {
    if link_count == 0 || row >= bottom {
        return None;
    }
    let header = MenuState::clip_header_row(bottom, context, mount_count, link_count)?;
    if row <= header {
        return None;
    }
    let index = bottom.saturating_sub(1).saturating_sub(row) as usize;
    let visible = MenuState::clip_visible_count(bottom, context, mount_count, link_count);
    (index < visible).then_some(index)
}

fn menu_line(frame: &mut Frame, x: u16, y: u16, text: &str) {
    let fitted = fit_cells(text, 17);
    print_at(frame, x, y, &fitted, Style::default());
}

fn clip_cells(text: &str, max: usize) -> String {
    if text_cell_width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let suffix = if max >= 2 { "…" } else { "" };
    let budget = max.saturating_sub(text_cell_width(suffix));
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = crate::screen::terminal_cell_width(ch) as usize;
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push_str(suffix);
    out
}

fn pad_cells(text: &str, width: usize) -> String {
    let used = text_cell_width(text);
    format!("{}{}", text, " ".repeat(width.saturating_sub(used)))
}

fn fit_cells(text: &str, width: usize) -> String {
    let clipped = clip_cells(text, width);
    pad_cells(&clipped, width)
}

fn print_at(frame: &mut Frame, x: u16, y: u16, text: &str, style: Style) {
    frame.put_str(x, y, text, style);
}
