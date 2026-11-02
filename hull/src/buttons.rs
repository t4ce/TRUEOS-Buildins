//! Prepare window-local button outlines when page geometry changes.
use super::{layout::ButtonBox, viewport::Viewport};
use trueos::ui4_winit::ButtonRegion;

pub(super) fn regions(buttons: &[ButtonBox], viewport: &Viewport) -> Vec<ButtonRegion> {
    let (ox, oy) = viewport.origin();
    let (sx, sy) = viewport.offset();
    let (w, h) = viewport.extent();
    buttons
        .iter()
        .filter_map(|button| {
            viewport.clip_rect(button.rect)?;
            let [x, y, width, height] = button.rect;
            if width > 8190.0 || height > 8190.0 {
                return None;
            }
            Some(ButtonRegion {
                x: (f64::from(x) - f64::from(sx) + f64::from(ox)).floor() as i32 - 1,
                y: (f64::from(y) - f64::from(sy) + f64::from(oy)).floor() as i32 - 1,
                width: width.ceil() as u32 + 2,
                height: height.ceil() as u32 + 2,
                clip_x: ox as i32,
                clip_y: oy as i32,
                clip_width: w,
                clip_height: h,
                border_rgba: u32::from_le_bytes(button.border_color),
            })
        })
        .take(256)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_keeps_original_edges_and_a_separate_scroll_clip() {
        let mut viewport = Viewport::default();
        viewport.resize_with_origin(100, 80, 0, 20);
        viewport.set_document(100, 300);
        viewport.scroll_by(0.0, 15.0);
        let regions = regions(
            &[ButtonBox {
                rect: [10.0, 5.0, 60.0, 30.0],
                border_color: [12, 34, 56, 255],
            }],
            &viewport,
        );
        assert_eq!(regions.len(), 1);
        let r = regions[0];
        assert_eq!((r.x, r.y, r.width, r.height), (9, 9, 62, 32));
        assert_eq!(
            (r.clip_x, r.clip_y, r.clip_width, r.clip_height),
            (0, 20, 100, 60)
        );
        assert_eq!(r.border_rgba, u32::from_le_bytes([12, 34, 56, 255]));
    }
    #[test]
    fn offscreen_and_oversized_buttons_are_not_registered_and_visible_count_is_bounded() {
        let mut viewport = Viewport::default();
        viewport.resize_with_origin(100, 80, 0, 20);
        let button = ButtonBox {
            rect: [2.0, 2.0, 30.0, 20.0],
            border_color: [0, 0, 0, 255],
        };
        assert_eq!(regions(&vec![button; 300], &viewport).len(), 256);
        let offscreen = ButtonBox {
            rect: [200.0, 200.0, 30.0, 20.0],
            ..button
        };
        let oversized = ButtonBox {
            rect: [0.0, 0.0, 9000.0, 20.0],
            ..button
        };
        assert!(regions(&[offscreen, oversized], &viewport).is_empty());
    }
}
