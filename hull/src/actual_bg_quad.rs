//! Solid CSS backgrounds, independent of the diagnostic pink layout pass.
use super::{layout::BackgroundQuad, viewport::Viewport};
use trueos::ui4_winit::{Error, SceneTarget, SpriteCorner, SpriteQuad};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum QuadPass {
    PinkQuadDbg,
    #[default]
    ActualBgQuad,
}

impl QuadPass {
    pub fn toggle(&mut self) {
        *self = match self {
            Self::PinkQuadDbg => Self::ActualBgQuad,
            Self::ActualBgQuad => Self::PinkQuadDbg,
        };
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::PinkQuadDbg => "pinkQuadDbg",
            Self::ActualBgQuad => "actualBgQuad",
        }
    }
}

pub(super) fn draw(
    backgrounds: &[BackgroundQuad],
    viewport: &Viewport,
    target: &mut SceneTarget,
) -> Result<(), Error> {
    // Sprite draws require a GPU frame. Paint white first so the transparent
    // native-frame clear never changes the page's default background.
    let mut quads = vec![quad(
        viewport.content_rect().map(|value| value as f32),
        [255; 4],
    )];
    quads.extend(backgrounds.iter().filter_map(|background| {
        Some(quad(viewport.clip_rect(background.rect)?, background.color))
    }));
    target.begin_gpu_frame()?;
    target.draw_sprite_quads(&quads)
}

fn quad([x, y, width, height]: [f32; 4], color: [u8; 4]) -> SpriteQuad {
    SpriteQuad {
        sprite_id: 0,
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
        color_rgba: u32::from_le_bytes(color),
        source_over: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_backgrounds_still_produce_a_white_gpu_frame() {
        let mut viewport = Viewport::default();
        viewport.resize(100, 50);
        let mut target = SceneTarget::default();
        draw(&[], &viewport, &mut target).unwrap();
        assert!(target.gpu_frame);
        assert_eq!(target.draws.len(), 1);
        assert_eq!(target.draws[0].color_rgba, u32::MAX);
    }

    #[test]
    fn white_page_background_begins_below_window_chrome() {
        let mut viewport = Viewport::default();
        viewport.resize_with_origin(1280, 752, 0, 32);
        let mut target = SceneTarget::default();
        draw(&[], &viewport, &mut target).unwrap();
        let page = target.draws[0];
        assert_eq!((page.c0.x, page.c0.y), (0.0, 32.0));
        assert_eq!((page.c2.x, page.c2.y), (1280.0, 752.0));
    }

    #[test]
    fn clips_backgrounds_and_preserves_alpha_and_paint_order() {
        let mut viewport = Viewport::default();
        viewport.resize(100, 50);
        let backgrounds = [
            BackgroundQuad {
                rect: [-10.0, 0.0, 30.0, 20.0],
                color: [255, 0, 0, 128],
            },
            BackgroundQuad {
                rect: [0.0, 0.0, 10.0, 10.0],
                color: [0, 0, 255, 255],
            },
            BackgroundQuad {
                rect: [200.0, 0.0, 10.0, 10.0],
                color: [0, 255, 0, 255],
            },
        ];
        let mut target = SceneTarget::default();
        draw(&backgrounds, &viewport, &mut target).unwrap();
        assert!(target.gpu_frame);
        assert_eq!(target.draws.len(), 3);
        assert_eq!(target.draws[0].color_rgba, u32::MAX);
        assert_eq!(target.draws[0].c2.x, 100.0);
        assert_eq!(target.draws[0].c2.y, 50.0);
        assert_eq!(target.draws[1].c0.x, 0.0);
        assert_eq!(target.draws[1].c1.x, 20.0);
        for (quad, background) in target.draws.iter().skip(1).zip(backgrounds) {
            assert_eq!(quad.sprite_id, 0);
            assert_eq!(quad.color_rgba, u32::from_le_bytes(background.color));
            assert!(quad.source_over);
        }
        let mut pass = QuadPass::default();
        assert!(pass == QuadPass::ActualBgQuad);
        pass.toggle();
        assert!(pass == QuadPass::PinkQuadDbg);
        pass.toggle();
        assert!(pass == QuadPass::ActualBgQuad);
    }
}
