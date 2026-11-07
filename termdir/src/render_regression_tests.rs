use super::*;
use crate::screen::Renderer;
use alloc::{string::String, vec::Vec};
use crossterm::io::{self as transport, Write};

#[derive(Default, Debug)]
struct Output {
    bytes: Vec<u8>,
    writes: usize,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> transport::Result<usize> {
        self.writes += 1;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> transport::Result<()> {
        Ok(())
    }
}

#[test]
fn tree_and_radial_frames_preserve_original_bytes_and_batching() {
    let mut graph = GraphView::empty(Path::new("trueosfs:disc7/My directory"));
    for directory in 0..8 {
        let parent = graph.nodes.len();
        graph.nodes.push(FsNode {
            id: parent,
            parent: Some(0),
            path: graph.root.join(format!("folder-{directory}")),
            name: format!("folder-{directory}"),
            is_dir: true,
            content_type: trueos::content_identity::ContentTypeId::NONE,
            is_placeholder: false,
            is_removed: false,
            is_moved: false,
            hidden: false,
            depth: 1,
        });
        for child in 0..8 {
            let id = graph.nodes.len();
            let name = match child {
                0 => "日本語・📁.txt".to_string(),
                1 => "🗺 map with spaces.txt".to_string(),
                _ => format!("file-{directory}-{child}.tar.lz4"),
            };
            graph.nodes.push(FsNode {
                id,
                parent: Some(parent),
                path: graph.nodes[parent].path.join(&name),
                name,
                is_dir: child == 6,
                content_type: trueos::content_identity::ContentTypeId::BLOB,
                is_placeholder: child == 7,
                is_removed: directory == 0 && child == 4,
                is_moved: directory == 1 && child == 5,
                hidden: directory == 2 && child == 6,
                depth: 2,
            });
        }
    }
    let mut renderer = Renderer::default();
    let mut out = crate::BufWriter::with_capacity(64 * 1024, Output::default());
    for mode in [LayoutMode::Tree, LayoutMode::Radial] {
        graph.layout_mode = mode;
        for line_style in [
            LineStyle::Default,
            LineStyle::Straight,
            LineStyle::Elbow,
            LineStyle::SoftArc,
        ] {
            graph.line_style = line_style;
            graph.rebuild_layout();
            for (width, height) in [(120, 40), (80, 24), (200, 60)] {
                for (x, y) in [(0, 0), (-30, 10), (40, -15), (0, 0)] {
                    graph.camera_x = x;
                    graph.camera_y = y;
                    let mut frame = Frame::new(width, height);
                    graph.render_many(
                        &mut frame,
                        Viewport {
                            x: 2,
                            y: 1,
                            width: width - 4,
                            height: height - 2,
                        },
                        Some(3),
                        &[3, 4],
                        Some(5),
                        &[7, 8],
                        Some(10),
                    );
                    renderer.present(&mut out, frame).unwrap();
                }
                // Same-size terminal handoff must repaint the retained scene.
                renderer.invalidate();
            }
        }
    }
    let output = out.into_inner().unwrap();
    assert_eq!(output.writes, 96, "one native write per frame");
    let digest = sha256_hex(&output.bytes);
    std::println!(
        "render trace: {} bytes, {} writes, sha256 {}",
        output.bytes.len(),
        output.writes,
        digest
    );
    // Recorded from the original production renderer before the no_std port.
    assert_eq!(
        digest,
        "bb863efd38d22b7d372a26ea02a2c2dde20f820a0807d2dfacabb6696ef994db"
    );
}
