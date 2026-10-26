use crossterm::style::Color;

use crate::{
    graph_view::{GraphView, Viewport},
    screen::{Frame, Style},
};

#[derive(Clone, Copy, Debug)]
pub struct MinimapGeometry {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl MinimapGeometry {
    pub fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x
            && x < self.x.saturating_add(self.width)
            && y >= self.y
            && y < self.y.saturating_add(self.height)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CacheKey {
    revision: u64,
    inner_width: u16,
    inner_height: u16,
}

#[derive(Clone, Debug)]
struct Cache {
    key: CacheKey,
    glyphs: Vec<char>,
}

#[derive(Default)]
pub struct Minimap {
    cache: Option<Cache>,
}

impl Minimap {
    pub fn geometry(
        terminal_width: u16,
        terminal_height: u16,
        viewport: Viewport,
    ) -> Option<MinimapGeometry> {
        // One fifth in both axes preserves the terminal's own rectangle aspect.
        // Layout::current has already applied the application's sole
        // terminal-too-small gate; the minimap follows that decision.
        let width = (terminal_width / 5).max(8).min(viewport.width);
        let height = (terminal_height / 5).max(5).min(viewport.height);
        Some(MinimapGeometry {
            x: viewport.x,
            y: viewport.bottom().saturating_sub(height),
            width,
            height,
        })
    }

    /// Translate a click in the framed minimap to the corresponding world-space
    /// point. This uses the exact same sample bounds as the density raster, so a
    /// click is relative to what the user sees rather than to camera state.
    pub fn world_at(
        graph: &GraphView,
        geometry: MinimapGeometry,
        x: u16,
        y: u16,
    ) -> Option<(i32, i32)> {
        let inner_width = geometry.width.saturating_sub(2);
        let inner_height = geometry.height.saturating_sub(2);
        if inner_width == 0 || inner_height == 0 {
            return None;
        }

        let samples = graph.minimap_samples();
        let (min_x, max_x, min_y, max_y) = sample_bounds(&samples)?;

        // Border clicks snap to the nearest point inside the minimap instead of
        // becoming dead zones. This makes the complete visible map clickable.
        let local_x = x
            .saturating_sub(geometry.x.saturating_add(1))
            .min(inner_width.saturating_sub(1));
        let local_y = y
            .saturating_sub(geometry.y.saturating_add(1))
            .min(inner_height.saturating_sub(1));

        let world_x = interpolate_axis(local_x, inner_width, min_x, max_x);
        let world_y = interpolate_axis(local_y, inner_height, min_y, max_y);
        Some((world_x, world_y))
    }

    pub fn draw(&mut self, frame: &mut Frame, graph: &GraphView, viewport: Viewport) {
        let Some(g) = Self::geometry(frame.width(), frame.height(), viewport) else {
            return;
        };

        let inner_width = g.width.saturating_sub(2);
        let inner_height = g.height.saturating_sub(2);
        let key = CacheKey {
            revision: graph.scene_revision(),
            inner_width,
            inner_height,
        };

        let rebuild = self
            .cache
            .as_ref()
            .map(|cache| cache.key != key)
            .unwrap_or(true);
        if rebuild {
            self.cache = Some(Cache {
                key,
                glyphs: rasterize(graph, inner_width, inner_height),
            });
        }

        let style = Style::new(Color::Black, Color::Grey);
        frame.fill_rect(g.x, g.y, g.width, g.height, ' ', style);

        if g.width >= 2 {
            frame.put(g.x, g.y, '╒', style);
            frame.put(g.x + g.width - 1, g.y, '╕', style);
            frame.put(g.x, g.y + g.height - 1, '╘', style);
            frame.put(g.x + g.width - 1, g.y + g.height - 1, '╛', style);
            for x in g.x + 1..g.x + g.width - 1 {
                frame.put(x, g.y, '═', style);
                frame.put(x, g.y + g.height - 1, '═', style);
            }

            // Fixed title connected to the double horizontal rule, with the
            // same mixed single/double frame vocabulary as confirmation boxes.
            const TITLE: &str = "╡ M A P ╞";
            let title_width = TITLE.chars().count() as u16;
            if g.width >= title_width.saturating_add(4) {
                let title_x = g.x + (g.width.saturating_sub(title_width)) / 2;
                for (offset, ch) in TITLE.chars().enumerate() {
                    frame.put(title_x + offset as u16, g.y, ch, style);
                }
            }
        }
        for y in g.y + 1..g.y + g.height - 1 {
            frame.put(g.x, y, '│', style);
            frame.put(g.x + g.width - 1, y, '│', style);
        }

        let Some(cache) = self.cache.as_ref() else {
            return;
        };
        for row in 0..inner_height {
            for column in 0..inner_width {
                let index = row as usize * inner_width as usize + column as usize;
                let ch = cache.glyphs.get(index).copied().unwrap_or(' ');
                if ch != ' ' {
                    frame.put(g.x + 1 + column, g.y + 1 + row, ch, style);
                }
            }
        }
    }
}

fn rasterize(graph: &GraphView, width: u16, height: u16) -> Vec<char> {
    let len = width as usize * height as usize;
    if width == 0 || height == 0 {
        return vec![' '; len];
    }

    let samples = graph.minimap_samples();
    if samples.is_empty() {
        return vec![' '; len];
    }

    let Some((min_x, max_x, min_y, max_y)) = sample_bounds(&samples) else {
        return vec![' '; len];
    };

    let dot_width = (width as i32 * 2).max(1);
    let dot_height = (height as i32 * 4).max(1);
    let span_x = (max_x - min_x).max(1) as i64;
    let span_y = (max_y - min_y).max(1) as i64;
    let mut masks = vec![0_u8; len];

    for (sample_index, &(x, y)) in samples.iter().enumerate() {
        let dx = if max_x == min_x {
            dot_width / 2
        } else {
            (((x - min_x) as i64 * (dot_width - 1) as i64) / span_x) as i32
        }
        .clamp(0, dot_width - 1);
        let dy = if max_y == min_y {
            dot_height / 2
        } else {
            (((y - min_y) as i64 * (dot_height - 1) as i64) / span_y) as i32
        }
        .clamp(0, dot_height - 1);

        let cell_x = (dx / 2) as u16;
        let cell_y = (dy / 4) as u16;
        let index = cell_y as usize * width as usize + cell_x as usize;
        let bit = braille_bit((dx & 1) as usize, (dy & 3) as usize);
        add_density_dot(&mut masks[index], bit, sample_index);
    }

    masks
        .into_iter()
        .map(|mask| {
            if mask == 0 {
                ' '
            } else {
                char::from_u32(0x2800 + mask as u32).unwrap_or(' ')
            }
        })
        .collect()
}

fn sample_bounds(samples: &[(i32, i32)]) -> Option<(i32, i32, i32, i32)> {
    let &(first_x, first_y) = samples.first()?;
    let (mut min_x, mut max_x) = (first_x, first_x);
    let (mut min_y, mut max_y) = (first_y, first_y);
    for &(x, y) in &samples[1..] {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    Some((min_x, max_x, min_y, max_y))
}

fn interpolate_axis(local: u16, cells: u16, min: i32, max: i32) -> i32 {
    if max == min || cells <= 1 {
        return min + (max - min) / 2;
    }
    let denominator = cells.saturating_sub(1) as i64;
    let span = (max - min) as i64;
    min + ((local as i64 * span) / denominator) as i32
}

fn braille_bit(x: usize, y: usize) -> u8 {
    const BITS: [[u8; 2]; 4] = [
        [1 << 0, 1 << 3],
        [1 << 1, 1 << 4],
        [1 << 2, 1 << 5],
        [1 << 6, 1 << 7],
    ];
    BITS[y.min(3)][x.min(1)]
}

fn add_density_dot(mask: &mut u8, preferred: u8, salt: usize) {
    if *mask & preferred == 0 {
        *mask |= preferred;
        return;
    }

    // If several graph samples collapse onto exactly the same minimap dot,
    // consume another free dot in the cell. Dense areas therefore become fuller
    // Braille glyphs instead of silently aliasing into a single point.
    for offset in 0..8 {
        let bit = 1_u8 << ((salt + offset) & 7);
        if *mask & bit == 0 {
            *mask |= bit;
            return;
        }
    }
}
