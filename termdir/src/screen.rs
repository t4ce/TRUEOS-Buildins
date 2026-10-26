use std::io::{self, Write};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
    terminal::{Clear, ClearType},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub underline: bool,
}

impl Style {
    pub const fn new(fg: Color, bg: Color) -> Self {
        Self {
            fg,
            bg,
            bold: false,
            underline: false,
        }
    }

    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub const fn underline(mut self) -> Self {
        self.underline = true;
        self
    }
}

impl Default for Style {
    fn default() -> Self {
        Self::new(Color::Reset, Color::Reset)
    }
}

// Crossterm deliberately does not ship a Unicode-width table. The UI only has
// a tiny set of glyphs whose display width matters to retained rendering.
// Keep the table intentionally local and dependency-free.
pub fn terminal_cell_width(ch: char) -> u16 {
    match ch {
        // Shell2's terminal treats the Japanese middle dot as wide. It is
        // used in termdir's slash-free breadcrumbs and title delimiters, so it
        // must occupy the same two cells in retained frames and hit layouts.
        '🪦' | '☰' | 'Ｎ' | 'Ｕ' | '＃' | '・' | '⌨' => 2,
        '🗺' => 3,
        _ => 1,
    }
}

pub fn text_cell_width(text: &str) -> usize {
    text.chars()
        .map(|ch| terminal_cell_width(ch) as usize)
        .sum()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
    // >1 = lead cell with glyph width, 1 = ordinary glyph, 0 = continuation.
    width: u8,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            style: Style::default(),
            width: 1,
        }
    }
}

impl Cell {
    fn wide(ch: char, style: Style, width: u16) -> Self {
        Self {
            ch,
            style,
            width: width as u8,
        }
    }

    fn continuation(style: Style) -> Self {
        Self {
            ch: ' ',
            style,
            width: 0,
        }
    }

    pub(crate) fn is_continuation(self) -> bool {
        self.width == 0
    }

    fn is_wide(self) -> bool {
        self.width > 1
    }
}

#[derive(Clone, Debug)]
pub struct Frame {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
}

impl Frame {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell::default(); width as usize * height as usize],
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    fn index(&self, x: u16, y: u16) -> usize {
        y as usize * self.width as usize + x as usize
    }

    fn erase_occupant(&mut self, x: u16, y: u16) {
        if x >= self.width || y >= self.height {
            return;
        }
        let mut lead_x = x;
        while lead_x > 0 && self.cell(lead_x, y).is_continuation() {
            lead_x -= 1;
        }
        let cell = self.cell(lead_x, y);
        if cell.is_wide() {
            for column in lead_x..lead_x.saturating_add(cell.width as u16).min(self.width) {
                let index = self.index(column, y);
                self.cells[index] = Cell::default();
            }
        }
    }

    pub fn put(&mut self, x: u16, y: u16, ch: char, style: Style) {
        if x >= self.width || y >= self.height {
            return;
        }
        self.erase_occupant(x, y);
        let index = self.index(x, y);
        self.cells[index] = Cell {
            ch,
            style,
            width: 1,
        };
    }

    pub fn put_display_char(&mut self, x: u16, y: u16, ch: char, style: Style) -> u16 {
        if x >= self.width || y >= self.height {
            return terminal_cell_width(ch);
        }

        let wanted = terminal_cell_width(ch);
        if wanted > 1 {
            if wanted > self.width - x {
                // Do not emit a partial wide icon at the right edge.
                for column in x..self.width { self.put(column, y, ' ', style); }
                return wanted;
            }
            for column in x..x + wanted { self.erase_occupant(column, y); }
            let lead = self.index(x, y);
            self.cells[lead] = Cell::wide(ch, style, wanted);
            for column in x + 1..x + wanted {
                let index = self.index(column, y);
                self.cells[index] = Cell::continuation(style);
            }
            wanted
        } else {
            self.put(x, y, ch, style);
            1
        }
    }

    pub fn put_i32(&mut self, x: i32, y: i32, ch: char, style: Style) {
        if x < 0 || y < 0 {
            return;
        }
        self.put(x as u16, y as u16, ch, style);
    }

    pub fn put_display_i32(&mut self, x: i32, y: i32, ch: char, style: Style) -> u16 {
        if x < 0 || y < 0 {
            return terminal_cell_width(ch);
        }
        self.put_display_char(x as u16, y as u16, ch, style)
    }

    pub fn put_str(&mut self, x: u16, y: u16, text: &str, style: Style) {
        if y >= self.height {
            return;
        }
        let mut column = x;
        for ch in text.chars() {
            if column >= self.width {
                break;
            }
            let advance = self.put_display_char(column, y, ch, style);
            column = column.saturating_add(advance);
        }
    }

    pub fn fill_rect(&mut self, x: u16, y: u16, width: u16, height: u16, ch: char, style: Style) {
        let right = x.saturating_add(width).min(self.width);
        let bottom = y.saturating_add(height).min(self.height);
        for row in y..bottom {
            for column in x..right {
                self.put(column, row, ch, style);
            }
        }
    }

    pub(crate) fn cell(&self, x: u16, y: u16) -> Cell {
        self.cells[self.index(x, y)]
    }
}

#[derive(Default)]
pub struct Renderer {
    previous: Option<Frame>,
}

impl Renderer {
    pub fn present<W: Write>(&mut self, out: &mut W, next: Frame) -> io::Result<()> {
        match self.previous.as_ref() {
            None => {
                queue!(out, ResetColor, MoveTo(0, 0), Clear(ClearType::All))?;
                self.paint_nonblank(out, &next)?;
            }
            Some(previous)
                if previous.width() != next.width() || previous.height() != next.height() =>
            {
                self.paint_all(out, &next)?;
            }
            Some(previous) => self.paint_diff(out, previous, &next)?,
        }

        queue!(out, ResetColor, SetAttribute(Attribute::Reset))?;
        out.flush()?;
        self.previous = Some(next);
        Ok(())
    }

    fn paint_nonblank<W: Write>(&self, out: &mut W, frame: &Frame) -> io::Result<()> {
        for y in 0..frame.height() {
            let mut x = 0;
            while x < frame.width() {
                let cell = frame.cell(x, y);
                if cell.is_continuation() {
                    x += 1;
                    continue;
                }
                if cell.is_wide() {
                    if cell != Cell::default() {
                        paint_run(out, x, y, cell.style, &cell.ch.to_string())?;
                    }
                    x = x.saturating_add(cell.width as u16);
                    continue;
                }
                if cell == Cell::default() {
                    x += 1;
                    continue;
                }

                let start = x;
                let style = cell.style;
                let mut text = String::new();
                while x < frame.width() {
                    let candidate = frame.cell(x, y);
                    if candidate == Cell::default()
                        || candidate.style != style
                        || candidate.is_continuation()
                        || candidate.is_wide()
                    {
                        break;
                    }
                    text.push(candidate.ch);
                    x += 1;
                }
                paint_run(out, start, y, style, &text)?;
            }
        }
        Ok(())
    }

    fn paint_all<W: Write>(&self, out: &mut W, frame: &Frame) -> io::Result<()> {
        for y in 0..frame.height() {
            self.paint_range(out, frame, y, 0, frame.width())?;
        }
        Ok(())
    }

    fn paint_diff<W: Write>(&self, out: &mut W, previous: &Frame, next: &Frame) -> io::Result<()> {
        let width = next.width() as usize;
        let mut dirty = vec![false; width];
        for y in 0..next.height() {
            dirty.fill(false);

            for x in 0..next.width() {
                if previous.cell(x, y) != next.cell(x, y) {
                    mark_footprint(&mut dirty, previous, x, y);
                    mark_footprint(&mut dirty, next, x, y);
                }
            }

            let mut x = 0usize;
            while x < width {
                if !dirty[x] {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < width && dirty[x] {
                    x += 1;
                }
                self.paint_range(out, next, y, start as u16, x as u16)?;
            }
        }
        Ok(())
    }

    fn paint_range<W: Write>(
        &self,
        out: &mut W,
        frame: &Frame,
        y: u16,
        start: u16,
        end: u16,
    ) -> io::Result<()> {
        let mut x = start;
        while x < end {
            let cell = frame.cell(x, y);
            if cell.is_continuation() {
                x += 1;
                continue;
            }
            if cell.is_wide() {
                paint_run(out, x, y, cell.style, &cell.ch.to_string())?;
                x = x.saturating_add(cell.width as u16);
                continue;
            }

            let run_start = x;
            let style = cell.style;
            let mut text = String::new();
            while x < end {
                let candidate = frame.cell(x, y);
                if candidate.style != style || candidate.is_continuation() || candidate.is_wide() {
                    break;
                }
                text.push(candidate.ch);
                x += 1;
            }
            paint_run(out, run_start, y, style, &text)?;
        }
        Ok(())
    }
}

fn mark_footprint(dirty: &mut [bool], frame: &Frame, x: u16, y: u16) {
    let mut lead_x = x;
    while lead_x > 0 && frame.cell(lead_x, y).is_continuation() {
        lead_x -= 1;
    }
    let cell = frame.cell(lead_x, y);
    let end = lead_x.saturating_add(u16::from(cell.width).max(1)).min(frame.width());
    for column in lead_x..end { dirty[column as usize] = true; }
}

fn paint_run<W: Write>(out: &mut W, x: u16, y: u16, style: Style, text: &str) -> io::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    queue!(
        out,
        MoveTo(x, y),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(style.fg),
        SetBackgroundColor(style.bg),
    )?;
    if style.bold {
        queue!(out, SetAttribute(Attribute::Bold))?;
    }
    if style.underline {
        queue!(out, SetAttribute(Attribute::Underlined))?;
    }
    queue!(out, Print(text))
}

#[cfg(test)]
mod tests {
    use super::{mark_footprint, terminal_cell_width, text_cell_width, Frame, Renderer, Style};
    use std::io::{self, BufWriter, Write};

    #[derive(Debug, Default)]
    struct RecordingWriter {
        writes: usize,
        bytes: Vec<u8>,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn presents_a_small_frame_in_one_buffered_write() {
        let mut frame = Frame::new(8, 1);
        frame.put_str(0, 0, "td", Style::default());

        let mut renderer = Renderer::default();
        let mut output = BufWriter::with_capacity(64 * 1024, RecordingWriter::default());
        renderer.present(&mut output, frame).unwrap();

        let output = output.into_inner().unwrap();
        assert_eq!(output.writes, 1);
        assert!(output.bytes.windows(2).any(|window| window == b"td"));
    }

    #[test]
    fn map_occupies_three_cells_and_clears_its_complete_footprint() {
        assert_eq!(text_cell_width("🗺  "), 5);
        for overwrite_x in 0..3 {
            let mut frame = Frame::new(6, 1);
            frame.put_str(0, 0, "🗺x", Style::default());
            assert_eq!(frame.cell(0, 0).width, 3);
            assert!(frame.cell(1, 0).is_continuation());
            assert!(frame.cell(2, 0).is_continuation());
            assert_eq!(frame.cell(3, 0).ch, 'x');
            let mut dirty = [false; 6];
            mark_footprint(&mut dirty, &frame, overwrite_x, 0);
            assert_eq!(dirty, [true, true, true, false, false, false]);
            frame.put(overwrite_x, 0, 'A', Style::default());
            for x in 0..3 {
                assert_eq!(frame.cell(x, 0).ch, if x == overwrite_x { 'A' } else { ' ' });
                assert!(!frame.cell(x, 0).is_continuation());
            }
        }
        let mut clipped = Frame::new(2, 1);
        clipped.put_str(0, 0, "🗺", Style::default());
        assert_eq!(clipped.cell(0, 0).ch, ' ');
        assert_eq!(clipped.cell(1, 0).ch, ' ');
    }

    #[test]
    fn japanese_middle_dot_occupies_two_cells_in_a_frame() {
        let mut frame = Frame::new(4, 1);
        let style = Style::default().underline();
        frame.put_str(0, 0, "・x", style);

        assert_eq!(terminal_cell_width('・'), 2);
        assert_eq!(text_cell_width("・・"), 4);
        assert!(frame.cell(0, 0).style.underline);
        assert!(frame.cell(1, 0).is_continuation());
        assert!(frame.cell(1, 0).style.underline);
        assert_eq!(frame.cell(2, 0).ch, 'x');
    }
}
