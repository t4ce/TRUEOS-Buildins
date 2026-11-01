//! A closed native loop: four square corners, or one chord per rounded corner.
use super::{layout::BorderQuad, viewport::Viewport};

pub(super) struct LoopDraw {
    pub first_index: u32,
    pub base_vertex: i32,
    pub count: u32,
    pub color: [u8; 4],
}

#[derive(Default)]
pub(super) struct Mesh {
    pub vertices: Vec<u8>,
    pub indices: Vec<u8>,
    pub draws: Vec<LoopDraw>,
}

pub(super) fn geometry(borders: &[BorderQuad], viewport: &Viewport) -> Mesh {
    let (width, height) = viewport.frame_extent();
    let (origin_x, origin_y) = viewport.origin();
    let (scroll_x, scroll_y) = viewport.offset();
    let mut mesh = Mesh::default();
    if width == 0 || height == 0 {
        return mesh;
    }
    for border in borders {
        let [x, y, w, h] = border.rect;
        // Keep the real boundary, including offscreen corners. Clipping the
        // rectangle first would invent borders along the viewport edges.
        let expanded = [x - 1.5, y - 1.5, w + 3.0, h + 3.0];
        if w <= 0.0
            || h <= 0.0
            || border.color[3] == 0
            || ![x, y, w, h, x + w, y + h].iter().all(|v| v.is_finite())
            || viewport.clip_rect(expanded).is_none()
        {
            continue;
        }
        let mut radii = border.radii.map(|[rx, ry]| {
            if rx.is_finite() && ry.is_finite() && rx > 0.0 && ry > 0.0 {
                [rx, ry]
            } else {
                [0.0; 2]
            }
        });
        // CSS overlap reduction: scale all corner radii together.
        let mut scale = 1.0f32;
        for (extent, sum) in [
            (w, radii[0][0] + radii[1][0]),
            (w, radii[3][0] + radii[2][0]),
            (h, radii[0][1] + radii[3][1]),
            (h, radii[1][1] + radii[2][1]),
        ] {
            if sum > 0.0 {
                scale = scale.min(extent / sum);
            }
        }
        for radius in &mut radii {
            radius[0] *= scale;
            radius[1] *= scale;
        }
        let base_vertex = (mesh.vertices.len() / 12) as i32;
        let first_index = (mesh.indices.len() / 4) as u32;
        let mut count = 0u32;
        for (corner, [rx, ry]) in radii.into_iter().enumerate() {
            let points = match corner {
                0 => [[x, y + ry], [x + rx, y]],
                1 => [[x + w - rx, y], [x + w, y + ry]],
                2 => [[x + w, y + h - ry], [x + w - rx, y + h]],
                _ => [[x + rx, y + h], [x, y + h - ry]],
            };
            // Zero radius contributes one corner. A radius contributes two
            // endpoints and exactly one straight chord, never an arc mesh.
            let n = if rx == 0.0 || ry == 0.0 { 1 } else { 2 };
            for [px, py] in points.into_iter().take(n) {
                for value in [
                    2.0 * (px - scroll_x + origin_x as f32) / width as f32 - 1.0,
                    1.0 - 2.0 * (py - scroll_y + origin_y as f32) / height as f32,
                    0.0,
                ] {
                    mesh.vertices.extend_from_slice(&value.to_le_bytes());
                }
                mesh.indices.extend_from_slice(&count.to_le_bytes());
                count += 1;
            }
        }
        mesh.draws.push(LoopDraw {
            first_index,
            base_vertex,
            count,
            color: border.color,
        });
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    fn viewport() -> Viewport {
        let mut viewport = Viewport::default();
        viewport.resize(100, 100);
        viewport
    }
    fn border(radii: [[f32; 2]; 4]) -> BorderQuad {
        BorderQuad {
            rect: [10.0, 20.0, 60.0, 40.0],
            color: [0, 0, 0, 255],
            radii,
        }
    }
    fn points(mesh: &Mesh) -> Vec<[f32; 2]> {
        mesh.vertices
            .chunks_exact(12)
            .map(|p| {
                [
                    (f32::from_le_bytes(p[0..4].try_into().unwrap()) + 1.0) * 50.0,
                    (1.0 - f32::from_le_bytes(p[4..8].try_into().unwrap())) * 50.0,
                ]
            })
            .collect()
    }
    #[test]
    fn square_is_four_unrepeated_vertices_and_one_loop_per_box() {
        let mesh = geometry(&[border([[0.0; 2]; 4]); 2], &viewport());
        assert_eq!(mesh.vertices.len(), 8 * 12);
        assert_eq!(mesh.draws.len(), 2);
        assert_eq!(mesh.draws[0].count, 4);
        assert_eq!(mesh.draws[1].base_vertex, 4);
        assert_eq!(mesh.draws[1].first_index, 4);
        assert_eq!(
            mesh.indices[..16],
            [0u32, 1, 2, 3].map(u32::to_le_bytes).concat()
        );
        for (actual, expected) in points(&mesh).iter().take(4).zip([
            [10.0, 20.0],
            [70.0, 20.0],
            [70.0, 60.0],
            [10.0, 60.0],
        ]) {
            assert!(
                (actual[0] - expected[0]).abs() < 0.0001
                    && (actual[1] - expected[1]).abs() < 0.0001
            );
        }
    }

    #[test]
    fn frame_projection_moves_page_borders_below_chrome_without_scaling() {
        let mut viewport = Viewport::default();
        viewport.resize_with_origin(100, 132, 0, 32);
        viewport.set_document(200, 300);
        viewport.scroll_by(5.0, 10.0);
        let mesh = geometry(&[border([[0.0; 2]; 4])], &viewport);
        let points: Vec<_> = mesh
            .vertices
            .chunks_exact(12)
            .map(|point| {
                [
                    (f32::from_le_bytes(point[..4].try_into().unwrap()) + 1.0) * 50.0,
                    (1.0 - f32::from_le_bytes(point[4..8].try_into().unwrap())) * 66.0,
                ]
            })
            .collect();
        for (point, expected) in
            points
                .iter()
                .zip([[5.0, 42.0], [65.0, 42.0], [65.0, 82.0], [5.0, 82.0]])
        {
            assert!((point[0] - expected[0]).abs() < 0.0001);
            assert!((point[1] - expected[1]).abs() < 0.0001);
        }
    }
    #[test]
    fn rounded_corners_add_only_one_chord_each_and_scale_to_fit() {
        let mesh = geometry(&[border([[100.0; 2]; 4])], &viewport());
        assert_eq!(mesh.draws[0].count, 8);
        assert_eq!(mesh.indices.len(), 8 * 4);
        let p = points(&mesh);
        assert!((p[0][1] - 40.0).abs() < 0.0001);
        assert!((p[1][0] - 30.0).abs() < 0.0001);
        let mut radii = [[0.0; 2]; 4];
        radii[0] = [5.0, 10.0];
        assert_eq!(geometry(&[border(radii)], &viewport()).draws[0].count, 5);
    }
    #[test]
    fn offscreen_boundary_is_not_replaced_with_viewport_border() {
        let mut b = border([[0.0; 2]; 4]);
        b.rect = [-20.0, -10.0, 60.0, 40.0];
        let mesh = geometry(&[b], &viewport());
        let p = points(&mesh);
        assert!(p[0][0] < -19.99 && p[0][1] < -9.99);
        b.rect[0] = 200.0;
        assert!(geometry(&[b], &viewport()).draws.is_empty());
        b.rect[0] = 10.0;
        b.color[3] = 0;
        assert!(geometry(&[b], &viewport()).draws.is_empty());
    }
}
