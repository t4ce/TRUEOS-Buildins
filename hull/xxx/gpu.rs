use super::Rect;
use trueos::ui4_winit::{Error, SceneTarget};
use trueos::vgpu::*;

/// Owns GPU resources until the renderer is dropped, not until a frame is submitted.
pub struct Quads {
    device: Device,
    queue: Queue,
    shader: ShaderModule,
    pipeline: RenderPipeline,
    mesh: Option<(Buffer, Buffer)>,
    draws: Vec<IndexedBatchDrawV2>,
}

fn ui_error(code: i32) -> Error {
    if code == ERR_BUSY {
        Error::Busy
    } else {
        Error::Unknown(code)
    }
}

impl Quads {
    pub fn new() -> Result<Self, i32> {
        let device = Device::open(Capabilities::DEFAULT.union(Capabilities::PRESENT))?;
        let result = (|| {
            let queue = device.create_queue(QueueClass::Render)?;
            let shader = device
                .create_shader_module(SHADER_PACKAGE_CLIP_POSITION3_IMMEDIATE_RGBA_FNV1A64)?;
            let pipeline = device.create_render_pipeline(shader, 12, 0)?;
            Ok(Self {
                device,
                queue,
                shader,
                pipeline,
                mesh: None,
                draws: Vec::new(),
            })
        })();
        if result.is_err() {
            let _ = device.close();
        }
        result
    }

    /// Upload the visible clipped rectangles when layout, scroll or extent changes.
    /// Width/height are the frame viewport, never the full document's height.
    pub fn set_layout(&mut self, width: u32, height: u32, rectangles: &[Rect]) -> Result<(), i32> {
        if width == 0
            || height == 0
            || rectangles.len() > u32::MAX as usize / 6
            || rectangles
                .iter()
                .any(|&[x, y, w, h]| ![x, y, w, h, x + w, y + h].iter().all(|v| v.is_finite()))
        {
            return Err(ERR_IO);
        }
        let (vertices, indices) = super::geometry(width, height, rectangles);
        self.upload(&vertices, &indices)?;
        self.draws = if indices.is_empty() {
            Vec::new()
        } else {
            vec![IndexedBatchDrawV2 {
                index_count: (indices.len() / 4) as u32,
                rgba8_srgb: u32::from_le_bytes(super::DEFAULT_COLOR),
                topology: PRIMITIVE_TOPOLOGY_TRIANGLE_LIST,
                ..Default::default()
            }]
        };
        Ok(())
    }

    /// Upload ordered solid rectangles with an independent color per rectangle.
    /// Coordinates refer to the complete frame, including native window chrome.
    /// Call `draw_overlay` to preserve pixels already authored by page rendering.
    pub fn set_colored_rectangles(
        &mut self,
        width: u32,
        height: u32,
        rectangles: &[(super::Rect, [u8; 4])],
    ) -> Result<(), i32> {
        let (vertices, indices, draws) = colored_rectangles_geometry(width, height, rectangles)?;
        self.upload(&vertices, &indices)?;
        self.draws = draws;
        Ok(())
    }

    pub fn set_borders(
        &mut self,
        borders: &[crate::layout::BorderQuad],
        viewport: &crate::viewport::Viewport,
    ) -> Result<(), i32> {
        let mesh = crate::border_geometry::geometry(borders, viewport);
        self.upload(&mesh.vertices, &mesh.indices)?;
        self.draws = mesh
            .draws
            .into_iter()
            .map(|draw| IndexedBatchDrawV2 {
                index_count: draw.count,
                first_index: draw.first_index,
                base_vertex: draw.base_vertex,
                rgba8_srgb: u32::from_le_bytes(draw.color),
                topology: PRIMITIVE_TOPOLOGY_LINE_LOOP,
                ..Default::default()
            })
            .collect();
        Ok(())
    }

    fn upload(&mut self, vertices: &[u8], indices: &[u8]) -> Result<(), i32> {
        let replacement = if indices.is_empty() {
            None
        } else {
            let vb = self
                .device
                .create_buffer(vertices.len(), BUFFER_USAGE_MAP_WRITE | BUFFER_USAGE_VERTEX)?;
            let uploaded = (|| {
                let ib = self
                    .device
                    .create_buffer(indices.len(), BUFFER_USAGE_MAP_WRITE | BUFFER_USAGE_INDEX)?;
                let result = (|| {
                    if self.device.write_buffer(vb, 0, &vertices)? != vertices.len()
                        || self.device.write_buffer(ib, 0, &indices)? != indices.len()
                    {
                        return Err(ERR_IO);
                    }
                    Ok((vb, ib))
                })();
                if result.is_err() {
                    let _ = self.device.destroy_buffer(ib);
                }
                result
            })();
            if uploaded.is_err() {
                let _ = self.device.destroy_buffer(vb);
            }
            Some(uploaded?)
        };
        if let Some((vertices, indices)) = self.mesh.take() {
            let _ = self.device.destroy_buffer(indices);
            let _ = self.device.destroy_buffer(vertices);
        }
        self.mesh = replacement;
        Ok(())
    }

    /// Clear and draw the pink fill pass.
    pub fn draw(&self, target: &mut SceneTarget) -> Result<(), Error> {
        if self.mesh.is_none() {
            return target.begin(u32::MAX);
        }
        target.begin_gpu_frame()?;
        self.submit(target, false)
    }

    /// Native line-loop borders over either fill pass, without clearing color.
    pub fn draw_borders(&self, target: &mut SceneTarget) -> Result<(), Error> {
        self.draw_overlay(target)
    }

    /// Overlay the uploaded geometry on the current exact frame allocation.
    /// LOAD_COLOR retains completed GPU or flushed CPU page pixels and supplies
    /// a new resident render release for later sprite or text overlays.
    pub fn draw_overlay(&self, target: &mut SceneTarget) -> Result<(), Error> {
        self.submit(target, true)
    }

    fn submit(&self, target: &mut SceneTarget, overlay: bool) -> Result<(), Error> {
        let Some((vertices, indices)) = self.mesh else {
            return Ok(());
        };
        for batch in draw_batches(&self.draws, overlay) {
            let surface = self
                .device
                .acquire_ui4_surface(target.render_target())
                .map_err(ui_error)?;
            let point = self
                .device
                .submit_ui4_indexed_batch_v2(
                    self.queue,
                    surface,
                    self.pipeline,
                    vertices,
                    indices,
                    batch,
                )
                .map_err(ui_error)?;
            self.device
                .wait(self.queue, point.value)
                .map_err(ui_error)?;
        }
        Ok(())
    }
}

impl Drop for Quads {
    fn drop(&mut self) {
        // Draws retire before returning; resources are safe to release at teardown.
        if let Some((vertices, indices)) = self.mesh.take() {
            let _ = self.device.destroy_buffer(indices);
            let _ = self.device.destroy_buffer(vertices);
        }
        let _ = self.device.destroy_render_pipeline(self.pipeline);
        let _ = self.device.destroy_shader_module(self.shader);
        let _ = self.device.close();
    }
}

fn colored_rectangles_geometry(
    width: u32,
    height: u32,
    rectangles: &[(Rect, [u8; 4])],
) -> Result<(Vec<u8>, Vec<u8>, Vec<IndexedBatchDrawV2>), i32> {
    if width == 0
        || height == 0
        || rectangles.len() > u32::MAX as usize / 6
        || rectangles.iter().any(|&([x, y, w, h], _)| {
            w < 0.0 || h < 0.0 || ![x, y, w, h, x + w, y + h].iter().all(|v| v.is_finite())
        })
    {
        return Err(ERR_IO);
    }
    let mut boxes = Vec::with_capacity(rectangles.len());
    let mut draws = Vec::with_capacity(rectangles.len());
    for &(rect, color) in rectangles {
        if rect[2] == 0.0 || rect[3] == 0.0 {
            continue;
        }
        draws.push(IndexedBatchDrawV2 {
            first_index: draws.len() as u32 * 6,
            index_count: 6,
            // super::geometry already stores absolute indices into the shared
            // vertex buffer. A per-rectangle base_vertex would shift twice.
            base_vertex: 0,
            rgba8_srgb: u32::from_le_bytes(color),
            topology: PRIMITIVE_TOPOLOGY_TRIANGLE_LIST,
            ..Default::default()
        });
        boxes.push(rect);
    }
    let (vertices, indices) = super::geometry(width, height, &boxes);
    Ok((vertices, indices, draws))
}

/// Keep each native submission within the resident state slots. Continuations
/// retain earlier pixels, including border loops from previous submissions.
fn draw_batches(
    draws: &[IndexedBatchDrawV2],
    overlay: bool,
) -> impl Iterator<Item = IndexedDrawBatchV2> + '_ {
    draws
        .chunks(MAX_INDEXED_BATCH_V2_SUBMIT_DRAWS)
        .enumerate()
        .map(move |(part, draws)| {
            let mut batch = IndexedDrawBatchV2 {
                clear_rgba8_srgb: u32::MAX,
                draw_count: draws.len() as u32,
                ..Default::default()
            };
            batch.draws[..draws.len()].copy_from_slice(draws);
            if overlay || part != 0 {
                batch.draws[0].reserved |= INDEXED_BATCH_V2_LOAD_COLOR;
            }
            batch
        })
}

#[cfg(test)]
mod batch_tests {
    use super::*;

    #[test]
    fn colored_rectangles_keep_geometry_offsets_and_colors_after_skipping_empty_boxes() {
        let rectangles = [
            ([0., 0., 100., 32.], [90, 91, 92, 255]),
            ([10., 4., 0., 20.], [200, 0, 0, 255]),
            ([20., 4., 24., 24.], [0, 0, 0, 255]),
            ([21., 5., 22., 22.], [255, 255, 255, 255]),
        ];
        let (vertices, indices, draws) = colored_rectangles_geometry(100, 80, &rectangles).unwrap();
        assert_eq!(vertices.len(), 3 * 4 * 12);
        assert_eq!(indices.len(), 3 * 6 * 4);
        assert_eq!(draws.len(), 3);
        for (n, color) in [[90, 91, 92, 255], [0, 0, 0, 255], [255; 4]]
            .into_iter()
            .enumerate()
        {
            assert_eq!(draws[n].first_index, n as u32 * 6);
            assert_eq!(draws[n].base_vertex, 0);
            assert_eq!(draws[n].index_count, 6);
            assert_eq!(draws[n].rgba8_srgb, u32::from_le_bytes(color));
            assert_eq!(draws[n].topology, PRIMITIVE_TOPOLOGY_TRIANGLE_LIST);
            let decoded: Vec<_> = indices[n * 24..(n + 1) * 24]
                .chunks_exact(4)
                .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
                .collect();
            assert_eq!(
                decoded,
                [0, 1, 2, 0, 2, 3].map(|index| index + n as u32 * 4)
            );
        }
        let batch = draw_batches(&draws, true).next().unwrap();
        assert_eq!(batch.draws[0].reserved, INDEXED_BATCH_V2_LOAD_COLOR);
        let top = f32::from_le_bytes(vertices[4..8].try_into().unwrap());
        let bottom = f32::from_le_bytes(vertices[28..32].try_into().unwrap());
        assert_eq!(top, 1.0);
        assert!((bottom - 0.2).abs() < 1e-6);
    }

    #[test]
    fn colored_rectangles_reject_invalid_geometry_and_support_an_empty_overlay() {
        assert!(colored_rectangles_geometry(0, 80, &[]).is_err());
        assert!(colored_rectangles_geometry(100, 0, &[]).is_err());
        for rect in [
            [0., 0., -1., 5.],
            [0., 0., 5., -1.],
            [f32::NAN, 0., 5., 5.],
            [0., 0., f32::INFINITY, 5.],
        ] {
            assert!(colored_rectangles_geometry(100, 80, &[(rect, [255; 4])]).is_err());
        }
        let (vertices, indices, draws) = colored_rectangles_geometry(100, 80, &[]).unwrap();
        assert!(vertices.is_empty() && indices.is_empty() && draws.is_empty());
    }

    #[test]
    fn large_border_worklists_preserve_every_loop_and_prior_pixels() {
        for count in [0, 1, 340, 341, 600, 601, 10_000] {
            let draws: Vec<_> = (0..count)
                .map(|index| IndexedBatchDrawV2 {
                    first_index: index * 4,
                    base_vertex: (index * 4) as i32,
                    index_count: 4,
                    topology: PRIMITIVE_TOPOLOGY_LINE_LOOP,
                    rgba8_srgb: index,
                    ..Default::default()
                })
                .collect();
            let mut next = 0;
            for batch in draw_batches(&draws, true) {
                let len = batch.draw_count as usize;
                assert!((1..=340).contains(&len));
                assert_eq!(batch.draws[0].reserved, INDEXED_BATCH_V2_LOAD_COLOR);
                for (index, draw) in batch.draws[..len].iter().enumerate() {
                    let mut decoded = *draw;
                    decoded.reserved &= !INDEXED_BATCH_V2_LOAD_COLOR;
                    assert_eq!(decoded, draws[next + index]);
                }
                assert!(
                    batch.draws[len..]
                        .iter()
                        .all(|draw| *draw == Default::default())
                );
                next += len;
            }
            assert_eq!(next, count as usize);
        }
    }

    #[test]
    fn fill_clears_once_and_continuations_load_existing_pixels() {
        let draws = vec![IndexedBatchDrawV2::default(); 681];
        let flags: Vec<_> = draw_batches(&draws, false)
            .map(|batch| batch.draws[0].reserved)
            .collect();
        assert_eq!(
            flags,
            [0, INDEXED_BATCH_V2_LOAD_COLOR, INDEXED_BATCH_V2_LOAD_COLOR]
        );
    }
}
