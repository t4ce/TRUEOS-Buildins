//! Window chrome in frame coordinates, independent of the HTML layout and SVG.
use alloc::{string::{String, ToString}, vec::Vec, vec};
use trueos::ui4_winit::{HullTextRun, SpriteCorner, SpriteQuad};
use url::Url;

pub(super) const HEIGHT: u32 = 32;
pub(super) const INITIAL_CONTENT: (u32, u32) = (1280, 720);
const BACKGROUND: [u8; 4] = [232, 235, 239, 255];
const BORDER: [u8; 4] = [0, 0, 0, 255];

/// Opaque row, its default one-unit border, and the icon slot's shared divider.
/// Painted after page text so partially scrolled glyphs cannot cover chrome.
pub(super) fn rectangles(width: u32, height: u32) -> Vec<([f32; 4], [u8; 4])> {
    let height = height.min(HEIGHT) as f32;
    let width = width as f32;
    if width == 0.0 || height == 0.0 {
        return Vec::new();
    }
    vec![
        ([0.0, 0.0, width, height], BACKGROUND),
        ([0.0, 0.0, width, 1.0], BORDER),
        ([0.0, height - 1.0, width, 1.0], BORDER),
        ([0.0, 0.0, 1.0, height], BORDER),
        ([width - 1.0, 0.0, 1.0, height], BORDER),
        ([width.min(HEIGHT as f32) - 1.0, 0.0, 1.0, height], BORDER),
    ]
}

pub(super) fn domain_letter(page_url: &str) -> String {
    Url::parse(page_url)
        .ok()
        .and_then(|url| {
            let host = url.host_str()?;
            let host = host.strip_prefix("www.").unwrap_or(host);
            host.chars().find(char::is_ascii_alphanumeric)
        })
        .map(|letter| letter.to_ascii_uppercase().to_string())
        .unwrap_or_else(|| "?".into())
}

fn inner_slot(width: u32, height: u32) -> Option<(f32, f32)> {
    let width = width.min(HEIGHT).saturating_sub(4);
    let height = height.min(HEIGHT).saturating_sub(4);
    (width != 0 && height != 0).then_some((width as f32, height as f32))
}

pub(super) fn letter_run<'a>(width: u32, height: u32, letter: &'a str) -> Option<HullTextRun<'a>> {
    let (inner_width, inner_height) = inner_slot(width, height)?;
    let size = inner_height.min(22.0).min(inner_width * 11.0 / 6.0);
    let advance = size * 6.0 / 11.0;
    Some(HullTextRun {
        text: letter,
        x: 2.0 + (inner_width - advance) * 0.5,
        // Microfont's alphabetic baseline is nine rows into its eleven-row em.
        baseline_y: 2.0 + (inner_height - size) * 0.5 + size * 9.0 / 11.0,
        size_px: size,
        advance_px: advance,
        rgba: u32::from_le_bytes(BORDER),
    })
}

pub(super) fn icon_quad(
    width: u32,
    height: u32,
    sprite_id: u32,
    source_width: u32,
    source_height: u32,
) -> Option<SpriteQuad> {
    let (inner_width, inner_height) = inner_slot(width, height)?;
    if sprite_id == 0 || source_width == 0 || source_height == 0 {
        return None;
    }
    let scale = (inner_width / source_width as f32).min(inner_height / source_height as f32);
    let width = source_width as f32 * scale;
    let height = source_height as f32 * scale;
    let x = 2.0 + (inner_width - width) * 0.5;
    let y = 2.0 + (inner_height - height) * 0.5;
    Some(SpriteQuad {
        sprite_id,
        c0: SpriteCorner {
            x,
            y,
            u: 0.0,
            v: 0.0,
        },
        c1: SpriteCorner {
            x: x + width,
            y,
            u: 1.0,
            v: 0.0,
        },
        c2: SpriteCorner {
            x: x + width,
            y: y + height,
            u: 1.0,
            v: 1.0,
        },
        c3: SpriteCorner {
            x,
            y: y + height,
            u: 0.0,
            v: 1.0,
        },
        color_rgba: u32::MAX,
        source_over: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_and_slot_stay_in_frame_coordinates_at_every_extent() {
        for (width, height) in [(1280, 752), (640, 400), (7, 3), (1, 1)] {
            let rectangles = rectangles(width, height);
            assert_eq!(
                rectangles[0].0,
                [0.0, 0.0, width as f32, height.min(32) as f32]
            );
            assert_eq!(rectangles[0].1[3], 255);
            for ([x, y, w, h], _) in rectangles {
                assert!(x >= 0.0 && y >= 0.0 && w > 0.0 && h > 0.0);
                assert!(x + w <= width as f32 && y + h <= height.min(32) as f32);
            }
        }
        assert!(rectangles(0, 100).is_empty());
        assert!(rectangles(100, 0).is_empty());
    }

    #[test]
    fn domain_initial_is_one_uppercase_character_without_www() {
        assert_eq!(domain_letter("https://www.youtube.com/watch?v=test"), "Y");
        assert_eq!(domain_letter("https://crates.io/"), "C");
        assert_eq!(domain_letter("https://github.com/"), "G");
        assert_eq!(domain_letter("https://chat.deepseek.com/"), "C");
        assert_eq!(domain_letter("not a URL"), "?");
    }

    #[test]
    fn icon_sizes_fit_inside_the_border_and_keep_their_aspect() {
        for (width, height) in [(16, 16), (256, 256), (256, 128), (1, 256)] {
            let q = icon_quad(1280, 752, 42, width, height).unwrap();
            let w = q.c1.x - q.c0.x;
            let h = q.c3.y - q.c0.y;
            assert!((w / h - width as f32 / height as f32).abs() < 1e-5);
            assert!(((q.c0.x + q.c2.x) * 0.5 - 16.0).abs() < 1e-5);
            assert!(((q.c0.y + q.c2.y) * 0.5 - 16.0).abs() < 1e-5);
            assert!(q.c0.x >= 2.0 && q.c0.y >= 2.0 && q.c2.x <= 30.0 && q.c2.y <= 30.0);
            assert!(q.source_over);
        }
        assert!(icon_quad(3, 3, 42, 16, 16).is_none());
        assert!(icon_quad(1280, 752, 0, 16, 16).is_none());
        assert!(icon_quad(1280, 752, 42, 0, 16).is_none());
    }

    #[test]
    fn fallback_uses_the_centered_microfont_cell() {
        let run = letter_run(1280, 752, "Y").unwrap();
        assert_eq!(run.text, "Y");
        assert_eq!(run.size_px, 22.0);
        assert_eq!(run.x + run.advance_px * 0.5, 16.0);
        let top = run.baseline_y - run.size_px * 9.0 / 11.0;
        assert_eq!(top + run.size_px * 0.5, 16.0);
        assert!(letter_run(3, 3, "Y").is_none());
    }
}
