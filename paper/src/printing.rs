//! Render the retained document at physical DIN A4 scale, independent of its window.

use alloc::{collections::BTreeMap, vec::Vec};
use paper::{Editor, Mode};
use trueos::print2d::{self, a4};

#[derive(Debug)]
pub enum Error {
    Page(print2d::Error),
    Font(i32),
    InvalidGlyph,
}

pub fn render_page(
    editor: &Editor,
    scale: u32,
    mut glyph: impl FnMut(u32, char) -> Result<Vec<u8>, i32>,
) -> Result<a4::Page, Error> {
    let mut page = a4::Page::white().map_err(Error::Page)?;
    let (width_mm, height_mm) = editor.sheet.dimensions();
    let offset_x = (210.0 - width_mm as f32) * 0.5;
    let offset_y = (297.0 - height_mm as f32) * 0.5;
    let (left, top, column_width, row_height) = editor.bounds_mm(0, scale);
    let left = left + offset_x;
    let top = top + offset_y;
    let right = left + editor.columns as f32 * column_width;
    let bottom = top + editor.rows as f32 * row_height;
    if editor.mode == Mode::Gridpaper {
        for column in 0..=editor.columns {
            vertical(&mut page, left + column as f32 * column_width, top, bottom);
        }
    }
    if editor.mode != Mode::Whitepaper {
        for row in 0..=editor.rows {
            horizontal(&mut page, top + row as f32 * row_height, left, right);
        }
    }
    let em_mm = 5.0
        * if editor.mode == Mode::Gridpaper {
            scale
        } else {
            1
        } as f32;
    let target_height = a4::mm_to_px(em_mm).max(1);
    let target_width = (target_height * 6 + 5) / 11;
    let glyph_scale = if editor.mode == Mode::Gridpaper {
        scale
    } else {
        1
    };
    let mut glyphs = BTreeMap::new();
    for (index, cell) in editor.cells.iter().enumerate() {
        let Some(character) = cell.character else {
            continue;
        };
        if !glyphs.contains_key(&character) {
            let bytes = glyph(glyph_scale, character).map_err(Error::Font)?;
            if bytes.len() < 8 {
                return Err(Error::InvalidGlyph);
            }
            let width = u32::from_le_bytes(bytes[..4].try_into().unwrap());
            let height = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
            let length = (width as usize)
                .checked_mul(height as usize)
                .and_then(|pixels| pixels.checked_mul(4))
                .and_then(|pixels| pixels.checked_add(8))
                .ok_or(Error::InvalidGlyph)?;
            if width == 0 || height == 0 || bytes.len() != length {
                return Err(Error::InvalidGlyph);
            }
            glyphs.insert(character, (width, height, bytes));
        }
        let (source_width, source_height, bytes) = &glyphs[&character];
        let (x, y, w, h) = editor.bounds_mm(index, scale);
        let center_x_mm = x + offset_x + w * 0.5;
        let center_y_mm = y + offset_y + h * 0.5;
        let x = signed_mm_to_px(center_x_mm) - target_width as i32 / 2;
        let y = signed_mm_to_px(center_y_mm) - target_height as i32 / 2;
        glyph_on_page(
            &mut page,
            &bytes[8..],
            *source_width,
            *source_height,
            x,
            y,
            target_width,
            target_height,
        );
    }
    Ok(page)
}

fn signed_mm_to_px(mm: f32) -> i32 {
    if mm < 0.0 {
        -(a4::mm_to_px(-mm) as i32)
    } else {
        a4::mm_to_px(mm) as i32
    }
}

fn vertical(page: &mut a4::Page, x: f32, top: f32, bottom: f32) {
    if !(0.0..=210.0).contains(&x) || bottom < 0.0 || top > 297.0 {
        return;
    }
    let x = a4::mm_to_px(x).min(a4::WIDTH - 1);
    page.line(
        x,
        a4::mm_to_px(top.max(0.0)).min(a4::HEIGHT - 1),
        x,
        a4::mm_to_px(bottom.min(297.0)).min(a4::HEIGHT - 1),
        [190; 3],
    );
}

fn horizontal(page: &mut a4::Page, y: f32, left: f32, right: f32) {
    if !(0.0..=297.0).contains(&y) || right < 0.0 || left > 210.0 {
        return;
    }
    let y = a4::mm_to_px(y).min(a4::HEIGHT - 1);
    page.line(
        a4::mm_to_px(left.max(0.0)).min(a4::WIDTH - 1),
        y,
        a4::mm_to_px(right.min(210.0)).min(a4::WIDTH - 1),
        y,
        [190; 3],
    );
}

fn glyph_on_page(
    page: &mut a4::Page,
    source: &[u8],
    source_width: u32,
    source_height: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) {
    let rgb = page.rgb_mut();
    for row in 0..height {
        let target_y = y + row as i32;
        if !(0..a4::HEIGHT as i32).contains(&target_y) {
            continue;
        }
        for column in 0..width {
            let target_x = x + column as i32;
            if !(0..a4::WIDTH as i32).contains(&target_x) {
                continue;
            }
            let from = ((row as usize * source_height as usize / height as usize)
                * source_width as usize
                + column as usize * source_width as usize / width as usize)
                * 4;
            let to = (target_y as usize * a4::WIDTH as usize + target_x as usize) * 3;
            let alpha = u32::from(source[from + 3]);
            // Font tiles have a white background. Treat their luminance as
            // coverage so a blank em border cannot erase the document ruling.
            let coverage = (255 - u32::from(source[from])) * alpha / 255;
            for channel in &mut rgb[to..to + 3] {
                *channel = ((u32::from(*channel) * (255 - coverage) + 127) / 255) as u8;
            }
        }
    }
}
