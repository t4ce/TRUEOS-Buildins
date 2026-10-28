#![no_std]
extern crate alloc;
use alloc::vec::Vec;

pub const COLUMNS: u32 = 39;
pub const ROWS: u32 = 55;
pub const INDEX_COUNT: u32 = (COLUMNS + ROWS + 2) * 2;

/// Physical sheet dimensions. Compact sheets use whole 5 mm units, with no
/// A4 margins. Bound storage to the original A4 cell budget.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sheet {
    #[default]
    A4,
    Compact {
        width_mm: u32,
        height_mm: u32,
    },
}
impl Sheet {
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            Self::A4 => (210, 297),
            Self::Compact {
                width_mm,
                height_mm,
            } => (width_mm, height_mm),
        }
    }
    fn grid_units(self) -> (u32, u32) {
        match self {
            Self::A4 => (COLUMNS, ROWS),
            Self::Compact {
                width_mm,
                height_mm,
            } => (width_mm / 5, height_mm / 5),
        }
    }
}

/// Page ruling is independent of row spacing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Gridpaper,
    Linepaper,
    Whitepaper,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub mode: Mode,
    pub scale: u32,
    pub sheet: Sheet,
    pub wrap: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Gridpaper,
            scale: 1,
            sheet: Sheet::A4,
            wrap: true,
        }
    }
}
/// Top-level launch settings. `step` and the original `scale` are aliases.
pub fn script_settings(script: &str) -> Result<Settings, &'static str> {
    let mut settings = Settings::default();
    for line in script.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut words = line.split_whitespace();
        let command = words.next().unwrap();
        let value = words.next().ok_or("missing setting value")?;
        if words.next().is_some() {
            return Err("unexpected setting argument");
        }
        match command {
            "mode" => {
                settings.mode = match value {
                    "gridpaper" => Mode::Gridpaper,
                    "linepaper" => Mode::Linepaper,
                    "whitepaper" => Mode::Whitepaper,
                    _ => return Err("expected mode gridpaper|linepaper|whitepaper"),
                }
            }
            "scale" | "step" => {
                settings.scale = value
                    .strip_suffix('x')
                    .unwrap_or(value)
                    .parse()
                    .map_err(|_| "invalid step")?;
                if !(1..=8).contains(&settings.scale) {
                    return Err("expected step 1x..8x");
                }
            }
            "size" => {
                settings.sheet = if value == "a4" {
                    Sheet::A4
                } else {
                    let (width, height) = value
                        .split_once('x')
                        .ok_or("expected size a4|WIDTHxHEIGHT in mm")?;
                    let width_mm: u32 = width.parse().map_err(|_| "invalid sheet width")?;
                    let height_mm: u32 = height.parse().map_err(|_| "invalid sheet height")?;
                    if !(5..=400).contains(&width_mm)
                        || !(5..=400).contains(&height_mm)
                        || width_mm % 5 != 0
                        || height_mm % 5 != 0
                        || (width_mm / 5) * (height_mm / 5) > COLUMNS * ROWS
                    {
                        return Err("size must use 5 mm steps within the A4 cell budget");
                    }
                    Sheet::Compact {
                        width_mm,
                        height_mm,
                    }
                };
            }
            "wrap" => {
                settings.wrap = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("expected wrap on|off"),
                }
            }
            _ => return Err("unknown paper setting"),
        }
    }
    let (columns, rows) = settings.sheet.grid_units();
    if columns < settings.scale || rows < settings.scale {
        return Err("step leaves no complete cells on the sheet");
    }
    Ok(settings)
}
pub fn script_scale(script: &str) -> Result<u32, &'static str> {
    script_settings(script).map(|settings| settings.scale)
}

/// Match Gridpaper's centered 195 x 275 mm grid on a full 210 x 297 mm page.
/// Keep physical sizing on resize: extra area is white, smaller windows clip.
/// Each independent segment is two vertices and two indices, never adjacency.
pub fn geometry(page: (u32, u32), viewport: (u32, u32), scale: u32) -> (Vec<u8>, Vec<u8>) {
    geometry_for_mode(page, viewport, scale, Mode::Gridpaper)
}

pub fn geometry_for_mode(
    page: (u32, u32),
    viewport: (u32, u32),
    scale: u32,
    mode: Mode,
) -> (Vec<u8>, Vec<u8>) {
    geometry_for_settings(
        page,
        viewport,
        Settings {
            mode,
            scale,
            ..Settings::default()
        },
    )
}

pub fn geometry_for_settings(
    page: (u32, u32),
    viewport: (u32, u32),
    settings: Settings,
) -> (Vec<u8>, Vec<u8>) {
    let layout = Layout::new(settings);
    let columns = layout.columns;
    let rows = layout.rows;
    let step = layout.row_step;
    let left = layout.left;
    let top = layout.top;
    let right = left + columns as f32 * layout.column_step;
    let bottom = top + rows as f32 * step;
    let (width_mm, height_mm) = settings.sheet.dimensions();
    let mut vertices = Vec::with_capacity(INDEX_COUNT as usize * 12);
    let mut indices = Vec::with_capacity(INDEX_COUNT as usize * 4);
    let mut vertex = |x_mm: f32, y_mm: f32| {
        // Snap to pixel centers for crisp single-sample one-pixel lines.
        let x = ((x_mm * page.0 as f32 / width_mm as f32) as u32).min(page.0.saturating_sub(1))
            as f32
            + 0.5;
        let y = ((y_mm * page.1 as f32 / height_mm as f32) as u32).min(page.1.saturating_sub(1))
            as f32
            + 0.5;
        for value in [
            2.0 * x / viewport.0 as f32 - 1.0,
            1.0 - 2.0 * y / viewport.1 as f32,
            0.0,
        ] {
            vertices.extend_from_slice(&value.to_le_bytes());
        }
        indices.extend_from_slice(&((indices.len() / 4) as u32).to_le_bytes());
    };
    if settings.mode == Mode::Gridpaper {
        for column in 0..=columns {
            let x = left + column as f32 * step;
            vertex(x, top);
            vertex(x, bottom);
        }
    }
    for row in 0..=rows {
        let y = top + row as f32 * step;
        vertex(left, y);
        vertex(right, y);
    }
    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modes_keep_columns_fixed_and_only_scale_row_height() {
        for mode in [Mode::Linepaper, Mode::Whitepaper] {
            let editor = Editor::with_mode(3, mode);
            assert_eq!(editor.columns, 39);
            assert_eq!(editor.rows, 18);
            let a = editor.bounds(0, (840, 1188), 3);
            let b = editor.bounds(1, (840, 1188), 3);
            assert_eq!(a.2, 20);
            assert_eq!(a.3, 60);
            assert_eq!(b.0 - a.0, 20);
            let (v, i) = geometry_for_mode((840, 1188), (840, 1188), 3, mode);
            assert_eq!(i.len(), 19 * 2 * 4);
            for line in v.chunks_exact(24) {
                assert_eq!(&line[4..8], &line[16..20]);
            }
        }
        let settings = script_settings("mode linepaper\nstep 3x").unwrap();
        assert_eq!(settings.mode, Mode::Linepaper);
        assert_eq!(settings.scale, 3);
        assert_eq!(
            script_settings("mode whitepaper").unwrap().mode,
            Mode::Whitepaper
        );
        for script in ["mode", "mode unknown", "mode linepaper extra", "step 0"] {
            assert!(script_settings(script).is_err());
        }
    }
    #[test]
    fn scales_parse_and_reduce_line_counts() {
        assert_eq!(script_scale("# default\n"), Ok(1));
        for scale in 1..=8 {
            let script = alloc::format!("scale {scale}x\n");
            assert_eq!(script_scale(&script), Ok(scale));
            assert_eq!(script_scale(&alloc::format!("scale {scale}")), Ok(scale));
            let (v, i) = geometry((840, 1188), (840, 1188), scale);
            let count = ((COLUMNS / scale + ROWS / scale + 2) * 2) as usize;
            assert_eq!(v.len(), count * 12);
            assert_eq!(i.len(), count * 4);
            let x = |n: usize| f32::from_le_bytes(v[n * 12..n * 12 + 4].try_into().unwrap());
            assert!(((x(2) - x(0)) * 420.0 - 20.0 * scale as f32).abs() < 0.001);
        }
        for script in [
            "scale 0",
            "scale 9x",
            "scale -1",
            "scale",
            "scale 2 extra",
            "zoom 2",
        ] {
            assert!(script_scale(script).is_err());
        }
    }
    #[test]
    fn centered_five_mm_grid_has_only_independent_line_pairs() {
        let (vertices, indices) = geometry((840, 1188), (840, 1188), 1);
        assert_eq!(INDEX_COUNT, 192);
        assert_eq!(vertices.len(), 192 * 12);
        assert_eq!(indices.len(), 192 * 4);
        let pixel = |n: usize, axis: usize| {
            let start = n * 12 + axis * 4;
            let clip = f32::from_le_bytes(vertices[start..start + 4].try_into().unwrap());
            if axis == 0 {
                (clip + 1.0) * 420.0
            } else {
                (1.0 - clip) * 594.0
            }
        };
        assert!((pixel(0, 0) - 30.5).abs() < 0.001);
        assert!((pixel(0, 1) - 44.5).abs() < 0.001);
        for column in 0..39 {
            assert!((pixel((column + 1) * 2, 0) - pixel(column * 2, 0) - 20.0).abs() < 0.001);
        }
        for (n, bytes) in indices.chunks_exact(4).enumerate() {
            assert_eq!(u32::from_le_bytes(bytes.try_into().unwrap()), n as u32);
        }
    }
}

/// One physical layout shared by ruling, glyph bounds, and pointer hit testing.
struct Layout {
    columns: u32,
    rows: u32,
    column_step: f32,
    row_step: f32,
    left: f32,
    top: f32,
}
impl Layout {
    fn new(settings: Settings) -> Self {
        assert!((1..=8).contains(&settings.scale));
        let (units_x, units_y) = settings.sheet.grid_units();
        let columns = if settings.mode == Mode::Gridpaper {
            units_x / settings.scale
        } else {
            units_x
        };
        let rows = units_y / settings.scale;
        assert!(columns > 0 && rows > 0);
        let column_step = if settings.mode == Mode::Gridpaper {
            5.0 * settings.scale as f32
        } else {
            5.0
        };
        let row_step = 5.0 * settings.scale as f32;
        let (width, height) = settings.sheet.dimensions();
        Self {
            columns,
            rows,
            column_step,
            row_step,
            left: (width as f32 - columns as f32 * column_step) / 2.0,
            top: (height as f32 - rows as f32 * row_step) / 2.0,
        }
    }
}

/// Fixed document storage. Colour is reserved per cell for future editing;
/// the current raster transport uses black on white.
#[derive(Clone, Copy, Default)]
pub struct Cell {
    pub character: Option<char>,
    pub foreground: u32,
}
pub struct Editor {
    pub cells: Vec<Cell>,
    pub cursor: usize,
    pub columns: usize,
    pub rows: usize,
    pub mode: Mode,
    pub sheet: Sheet,
    pub wrap: bool,
}
impl Editor {
    pub fn new(scale: u32) -> Self {
        Self::with_mode(scale, Mode::Gridpaper)
    }
    pub fn with_mode(scale: u32, mode: Mode) -> Self {
        Self::with_settings(Settings {
            scale,
            mode,
            ..Settings::default()
        })
    }
    pub fn with_settings(settings: Settings) -> Self {
        let layout = Layout::new(settings);
        let columns = layout.columns as usize;
        let rows = layout.rows as usize;
        Self {
            cells: alloc::vec![Cell::default(); columns * rows],
            cursor: 0,
            columns,
            rows,
            mode: settings.mode,
            sheet: settings.sheet,
            wrap: settings.wrap,
        }
    }
    pub fn advance(&mut self) {
        self.cursor = (self.cursor + 1).min(self.cells.len() - 1);
    }
    pub fn type_char(&mut self, ch: char) {
        if ch == ' ' {
            self.advance_typing();
        } else if !ch.is_control() {
            self.cells[self.cursor].character = Some(ch);
            self.advance_typing();
        }
    }
    fn advance_typing(&mut self) {
        if self.wrap || (self.cursor + 1) % self.columns != 0 {
            self.advance();
        }
    }
    pub fn key(&mut self, key: u16) {
        use trueos::input::*;
        match key {
            KEYBOARD_KEY_BACKSPACE => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.cells[self.cursor].character = None;
                }
            }
            KEYBOARD_KEY_DELETE => self.cells[self.cursor].character = None,
            KEYBOARD_KEY_SPACE => self.advance_typing(),
            KEYBOARD_KEY_ARROW_LEFT => self.cursor = self.cursor.saturating_sub(1),
            KEYBOARD_KEY_ARROW_RIGHT => self.advance(),
            KEYBOARD_KEY_ARROW_UP => {
                if self.cursor >= self.columns {
                    self.cursor -= self.columns;
                }
            }
            KEYBOARD_KEY_ARROW_DOWN => {
                if self.cursor + self.columns < self.cells.len() {
                    self.cursor += self.columns;
                }
            }
            _ => {} // Enter deliberately has no action.
        }
    }
    /// Pixel cell bounds aligned with the independently rendered grid lines.
    pub fn bounds(&self, index: usize, page: (u32, u32), scale: u32) -> (u32, u32, u32, u32) {
        let layout = Layout::new(Settings {
            scale,
            mode: self.mode,
            sheet: self.sheet,
            wrap: self.wrap,
        });
        let (width_mm, height_mm) = self.sheet.dimensions();
        // Integer cell edges match the pixel-center snapped ruling at every DPI.
        let x = (layout.left + (index % self.columns) as f32 * layout.column_step) * page.0 as f32
            / width_mm as f32;
        let y = (layout.top + (index / self.columns) as f32 * layout.row_step) * page.1 as f32
            / height_mm as f32;
        let right = (layout.left + (index % self.columns + 1) as f32 * layout.column_step)
            * page.0 as f32
            / width_mm as f32;
        let bottom = (layout.top + (index / self.columns + 1) as f32 * layout.row_step)
            * page.1 as f32
            / height_mm as f32;
        (
            x as u32,
            y as u32,
            right as u32 - x as u32,
            bottom as u32 - y as u32,
        )
    }
    pub fn click(&mut self, x: i32, y: i32, page: (u32, u32), scale: u32) -> bool {
        if x < 0 || y < 0 {
            return false;
        }
        for index in 0..self.cells.len() {
            let (left, top, w, h) = self.bounds(index, page, scale);
            if x as u32 >= left && (x as u32) < left + w && y as u32 >= top && (y as u32) < top + h
            {
                self.cursor = index;
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;
    use trueos::input::*;
    #[test]
    fn fixed_budget_and_editing() {
        for scale in 1..=8 {
            let mut e = Editor::new(scale);
            assert_eq!(e.cells.len(), (COLUMNS / scale * (ROWS / scale)) as usize);
            e.type_char('a');
            e.type_char(' ');
            e.type_char('b');
            assert_eq!(e.cursor, 3);
            e.key(KEYBOARD_KEY_ENTER);
            assert_eq!(e.cursor, 3);
            e.key(KEYBOARD_KEY_BACKSPACE);
            assert_eq!(e.cells[2].character, None);
            assert_eq!(e.cursor, 2);
            assert_eq!(e.cells[0].character, Some('a'));
            e.key(KEYBOARD_KEY_ARROW_DOWN);
            assert_eq!(e.cursor, 2 + e.columns);
            e.key(KEYBOARD_KEY_ARROW_UP);
            assert_eq!(e.cursor, 2);
            e.key(KEYBOARD_KEY_ARROW_LEFT);
            assert_eq!(e.cursor, 1);
            e.key(KEYBOARD_KEY_ARROW_RIGHT);
            assert_eq!(e.cursor, 2);
            assert!(e.click(420, 594, (840, 1188), scale));
            let previous = e.cursor;
            assert!(!e.click(-1, -1, (840, 1188), scale));
            assert_eq!(e.cursor, previous);
        }
    }
}

#[cfg(test)]
mod compact_tests {
    use super::*;
    use trueos::input::*;

    #[test]
    fn boot_greeting_click_type_and_erase_repeats() {
        let settings = script_settings("mode gridpaper\nsize 190x90\nstep 2\nwrap on\n").unwrap();
        let mut editor = Editor::with_settings(settings);
        assert_eq!((editor.columns, editor.rows), (19, 9));
        // Nonintegral pixels per 5 mm cell exercises EDID/DPI quantization.
        let page = (718, 340);
        for _ in 0..3 {
            editor.cursor = editor.cells.len() - 1;
            let (x, y, w, h) = editor.bounds(0, page, 2);
            assert!(editor.click((x + w / 2) as i32, (y + h / 2) as i32, page, 2));
            for ch in "Hello from TrueOS §".chars() {
                editor.type_char(ch);
            }
            assert_eq!(editor.cursor, 19);
            assert_eq!(editor.cells[18].character, Some('§'));
            assert_eq!(editor.cells[0].character, Some('H'));
            for _ in "Hello from TrueOS §".chars() {
                editor.key(KEYBOARD_KEY_BACKSPACE);
            }
            assert_eq!(editor.cursor, 0);
            assert!(editor.cells.iter().all(|cell| cell.character.is_none()));
        }
    }

    #[test]
    fn compact_modes_wrap_and_snap_the_same_bounds_as_ruling() {
        for mode in [Mode::Gridpaper, Mode::Linepaper, Mode::Whitepaper] {
            for scale in 1..=8 {
                let settings = Settings {
                    mode,
                    scale,
                    sheet: Sheet::Compact {
                        width_mm: 95,
                        height_mm: 45,
                    },
                    wrap: true,
                };
                let mut editor = Editor::with_settings(settings);
                let page = (359, 170);
                let (vertices, _) = geometry_for_settings(page, page, settings);
                // All borders, including the right/bottom edge, stay in frame.
                for vertex in vertices.chunks_exact(12) {
                    for axis in 0..2 {
                        let value =
                            f32::from_le_bytes(vertex[axis * 4..axis * 4 + 4].try_into().unwrap());
                        assert!((-1.0..=1.0).contains(&value));
                    }
                }
                for index in 0..editor.cells.len() {
                    let (x, y, w, h) = editor.bounds(index, page, scale);
                    assert!(w > 0 && h > 0 && x + w <= page.0 && y + h <= page.1);
                    assert!(editor.click((x + w / 2) as i32, (y + h / 2) as i32, page, scale));
                    assert_eq!(editor.cursor, index);
                    if index % editor.columns + 1 < editor.columns {
                        assert_eq!(x + w, editor.bounds(index + 1, page, scale).0);
                    }
                }
                editor.cursor = editor.columns - 1;
                editor.type_char('a');
                assert_eq!(editor.cursor, editor.columns.min(editor.cells.len() - 1));
                editor.wrap = false;
                editor.cursor = editor.columns - 1;
                editor.type_char('b');
                editor.type_char(' ');
                assert_eq!(editor.cursor, editor.columns - 1);
            }
        }
    }

    #[test]
    fn size_and_wrap_settings_are_bounded_and_keep_a4_default() {
        assert_eq!(script_settings("").unwrap(), Settings::default());
        assert_eq!(
            script_settings("size 190x90\nwrap off").unwrap().wrap,
            false
        );
        assert_eq!(
            script_settings("size 95x45\nsize a4").unwrap().sheet,
            Sheet::A4
        );
        for script in [
            "size",
            "size 96x45",
            "size 0x45",
            "size 400x400",
            "size 999999999999x5",
            "size 5x5\nstep 8",
            "wrap yes",
            "wrap on extra",
        ] {
            assert!(script_settings(script).is_err(), "{script}");
        }
    }
}
