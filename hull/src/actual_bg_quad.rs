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
    let quads: Vec<_> = backgrounds
        .iter()
        .filter_map(|background| {
            let [x, y, w, h] = viewport.clip_rect(background.rect)?;
            Some(SpriteQuad {
                // UI4's frame-owned white pixel produces a constant-color quad.
                sprite_id: 0,
                c0: SpriteCorner {
                    x,
                    y,
                    u: 0.0,
                    v: 0.0,
                },
                c1: SpriteCorner {
                    x: x + w,
                    y,
                    u: 1.0,
                    v: 0.0,
                },
                c2: SpriteCorner {
                    x: x + w,
                    y: y + h,
                    u: 1.0,
                    v: 1.0,
                },
                c3: SpriteCorner {
                    x,
                    y: y + h,
                    u: 0.0,
                    v: 1.0,
                },
                color_rgba: u32::from_le_bytes(background.color),
                source_over: true,
            })
        })
        .collect();
    target.begin(u32::MAX)?;
    if !quads.is_empty() {
        // UI4 splits large worklists without changing source-over paint order.
        target.draw_sprite_quads(&quads)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(target.draws.len(), 2);
        assert_eq!(target.draws[0].c0.x, 0.0);
        assert_eq!(target.draws[0].c1.x, 20.0);
        for (quad, background) in target.draws.iter().zip(backgrounds) {
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
