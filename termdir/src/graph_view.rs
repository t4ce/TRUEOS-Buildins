use crate::chronos::{SystemTime, elapsed_since};
use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
};
use trueos::async_fs;
use trueos::collections::HashMap;

use crossterm::style::Color;
use sha2::{Digest, Sha256};

use crate::{
    layout::{self, LayoutMode, LayoutNode, WorldPos},
    screen::{Frame, Style, terminal_cell_width, text_cell_width},
};

const HARD_MAX_DEPTH: usize = 256;
const DEPTH_LEVELS: [usize; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
const DEFAULT_DEPTH_LIMIT: usize = 4;
const MAX_CHILDREN_PER_DIR: usize = 256;
const MAX_VISIBLE_NODES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sha256Result {
    pub source: usize,
    pub digest: String,
    pub path: PathBuf,
}

fn path_to_utf8(path: &Path) -> io::Result<&str> {
    path.to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path is not valid UTF-8"))
}

fn trueos_fs_err(op: &str, err: i32) -> io::Error {
    if op == "archive" && err == trueos::archive::ERR_TOO_LARGE {
        return io::Error::other("archive exceeds a size, entry, or path resource limit (-7)");
    }
    io::Error::other(format!("{op} failed ({err})"))
}

fn trueos_exists(path: &Path) -> io::Result<bool> {
    let path = path_to_utf8(path)?;
    async_fs::block_on(async_fs::exists(path.as_bytes()))
        .map_err(|err| trueos_fs_err("exists", err))
}

fn trueos_is_dir(path: &Path) -> io::Result<bool> {
    let path = path_to_utf8(path)?;
    async_fs::block_on(async_fs::metadata(path.as_bytes()))
        .map(|metadata| metadata.is_dir())
        .map_err(|err| trueos_fs_err("metadata", err))
}

fn read_file_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let path = path_to_utf8(path)?;
    async_fs::block_on(async_fs::read_file(path.as_bytes()))
        .map_err(|err| trueos_fs_err("read_file", err))
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push(HEX[(byte >> 4) as usize] as char);
        hex.push(HEX[(byte & 0x0f) as usize] as char);
    }
    hex
}

fn archive_suffix(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    [".tar.lz4", ".tar.mz4"]
        .into_iter()
        .find(|suffix| lower.ends_with(suffix))
}

fn unique_archive_destination(
    preferred: &Path,
    keep_archive_extension: bool,
) -> io::Result<PathBuf> {
    if !trueos_exists(preferred)? {
        return Ok(preferred.to_path_buf());
    }
    let parent = preferred.parent().unwrap_or_else(|| Path::new(""));
    let name = preferred
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "archive output name is not UTF-8",
            )
        })?;
    let extension = if keep_archive_extension {
        archive_suffix(name).unwrap_or(".tar.lz4")
    } else {
        ""
    };
    let base = if extension.is_empty() {
        name
    } else {
        &name[..name.len() - extension.len()]
    };
    for suffix in 2..=999 {
        let name = if keep_archive_extension {
            format!("{base}-{suffix}{extension}")
        } else {
            format!("{base}-{suffix}")
        };
        let candidate = parent.join(name);
        if !trueos_exists(&candidate)? {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free archive output name",
    ))
}

fn trueos_rename(source: &Path, destination: &Path) -> io::Result<()> {
    let source = path_to_utf8(source)?;
    let destination = path_to_utf8(destination)?;
    async_fs::block_on(async_fs::rename(source.as_bytes(), destination.as_bytes()))
        .map_err(|err| trueos_fs_err("rename", err))
}

#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Viewport {
    pub fn contains(&self, x: u16, y: u16) -> bool {
        x >= self.x
            && x < self.x.saturating_add(self.width)
            && y >= self.y
            && y < self.y.saturating_add(self.height)
    }

    pub fn right(&self) -> u16 {
        self.x.saturating_add(self.width)
    }

    pub fn bottom(&self) -> u16 {
        self.y.saturating_add(self.height)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WorldBounds {
    pub min_x: i32,
    pub max_x: i32,
    pub min_y: i32,
    pub max_y: i32,
}

#[derive(Clone, Debug)]
pub struct FsNode {
    pub id: usize,
    pub parent: Option<usize>,
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub content_type: trueos::content_identity::ContentTypeId,
    pub is_placeholder: bool,
    pub is_removed: bool,
    pub is_moved: bool,
    hidden: bool,
    depth: usize,
}

pub fn is_editable_text_file(node: &FsNode) -> bool {
    !node.is_dir && !node.is_placeholder && !node.is_removed && !node.is_moved && !node.hidden
        && node.path.extension().and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
}

#[derive(Clone, Debug)]
pub struct SelectionStats {
    pub key: String,
    pub kind: String,
    pub size: String,
    pub modified: String,
    pub access: String,
    pub content_type: ContentTypeDisplay,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentTypeDisplay {
    pub raw_hex: String,
    pub decimal: String,
    pub canonical_name: String,
    pub mime: String,
    pub status: String,
}

pub fn format_content_type(id: trueos::content_identity::ContentTypeId) -> ContentTypeDisplay {
    let raw = id.raw();
    match trueos::content_identity::content_type_info(id) {
        Some(info) => ContentTypeDisplay {
            raw_hex: format!("0x{raw:08x}"),
            decimal: raw.to_string(),
            canonical_name: info.canonical_name.to_string(),
            mime: info.mime_type.to_string(),
            status: "registered".to_string(),
        },
        None if raw == 0 => ContentTypeDisplay {
            raw_hex: format!("0x{raw:08x}"),
            decimal: raw.to_string(),
            canonical_name: "NONE".to_string(),
            mime: "application/octet-stream".to_string(),
            status: "unregistered".to_string(),
        },
        None => ContentTypeDisplay {
            raw_hex: format!("0x{raw:08x}"),
            decimal: raw.to_string(),
            canonical_name: "FUTURE/UNKNOWN".to_string(),
            mime: "application/octet-stream".to_string(),
            status: "future".to_string(),
        },
    }
}

#[cfg(test)]
mod content_type_tests {
    use super::format_content_type;
    use trueos::content_identity::ContentTypeId;

    #[test]
    fn renders_registered_identity_without_guessing() {
        let rendered = format_content_type(ContentTypeId::UTF8_TEXT);
        assert_eq!(rendered.raw_hex, "0x00000002");
        assert_eq!(rendered.decimal, "2");
        assert_eq!(rendered.canonical_name, "UTF8_TEXT");
        assert_eq!(rendered.mime, "text/plain");
        assert_eq!(rendered.status, "registered");
    }

    #[test]
    fn distinguishes_none_and_future_raw_identities() {
        assert_eq!(
            format_content_type(ContentTypeId::NONE).status,
            "unregistered"
        );
        let future = format_content_type(ContentTypeId::from_raw(0xfeed_beef));
        assert_eq!(future.raw_hex, "0xfeedbeef");
        assert_eq!(future.decimal, "4276993775");
        assert_eq!(future.status, "future");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStyle {
    /// Preserve the existing connector choice: S-curve in tree layout and
    /// radial spline in radial layout.
    Default,
    Straight,
    Elbow,
    SoftArc,
}

impl LineStyle {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Straight => "straight",
            Self::Elbow => "elbow",
            Self::SoftArc => "soft-arc",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Default => Self::Straight,
            Self::Straight => Self::Elbow,
            Self::Elbow => Self::SoftArc,
            Self::SoftArc => Self::Default,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct EdgeCell {
    x: i32,
    y: i32,
    mask: u8,
}

struct PendingArchive {
    future:
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<trueos::archive::Report, i32>>>>,
    destination: String,
    label: &'static str,
    last_marker: std::time::Instant,
    source: Option<PathBuf>,
    progress: std::rc::Rc<std::cell::Cell<u32>>,
    started: std::time::Instant,
    next_poll: std::time::Instant,
    displayed_percent: Option<u32>,
}

fn unpack_label_percent(elapsed: std::time::Duration, percent: u32) -> Option<u32> {
    (elapsed.as_millis() % 4000 < 3000).then_some(percent.min(99))
}

pub struct GraphView {
    content_enabled: bool,
    root: PathBuf,
    nodes: Vec<FsNode>,
    positions: Vec<WorldPos>,
    edge_cells: Vec<EdgeCell>,
    camera_x: i32,
    camera_y: i32,
    column_gap: i32,
    layout_mode: LayoutMode,
    line_style: LineStyle,
    suppressed_parent_edges: Vec<bool>,
    depth_limit: usize,
    scene_revision: u64,
    pending_archives: Vec<PendingArchive>,
}

impl GraphView {
    pub fn from_current_dir() -> io::Result<Self> {
        let root = std::env::current_dir()?;
        Self::from_path(&root)
    }

    pub fn from_path(root: &Path) -> io::Result<Self> {
        let mut view = Self::new(root, true);
        view.reload()?;
        Ok(view)
    }

    /// Native chrome can navigate and operate on the root without building
    /// the graph until its dedicated pan renderer is available.
    pub fn empty(root: &Path) -> Self {
        let mut view = Self::new(root, false);
        view.reload().expect("empty graph reload performs no filesystem I/O");
        view
    }

    fn new(root: &Path, content_enabled: bool) -> Self {
        Self {
            content_enabled,
            root: root.to_path_buf(),
            nodes: Vec::new(),
            positions: Vec::new(),
            edge_cells: Vec::new(),
            camera_x: 0,
            camera_y: 0,
            column_gap: 18,
            layout_mode: LayoutMode::Tree,
            line_style: LineStyle::Default,
            suppressed_parent_edges: Vec::new(),
            depth_limit: DEFAULT_DEPTH_LIMIT,
            scene_revision: 0,
            pending_archives: Vec::new(),
        }
    }

    pub fn reload(&mut self) -> io::Result<()> {
        self.nodes.clear();

        let root_name = self
            .root
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| self.root.display().to_string());

        // The mount root remains structural only. It participates in hierarchy,
        // move validation, and layout centering, but is never rendered as a node.
        self.nodes.push(FsNode {
            id: 0,
            parent: None,
            path: self.root.clone(),
            name: root_name,
            is_dir: true,
            content_type: trueos::content_identity::ContentTypeId::NONE,
            is_placeholder: false,
            is_removed: false,
            is_moved: false,
            hidden: false,
            depth: 0,
        });

        if !self.content_enabled {
            self.rebuild_layout();
            return Ok(());
        }
        let root = self.root.clone();
        let mut remaining = MAX_VISIBLE_NODES;
        self.scan_dir(0, &root, 1, &mut remaining)?;
        self.rebuild_layout();
        Ok(())
    }

    pub fn mount_parent(&mut self) -> io::Result<bool> {
        let Some(parent) = self.root.parent().map(Path::to_path_buf) else {
            return Ok(false);
        };
        if parent == self.root {
            return Ok(false);
        }
        self.root = parent;
        self.center();
        self.reload()?;
        Ok(true)
    }

    pub fn mount_node(&mut self, id: usize) -> io::Result<()> {
        let node = self
            .node(id)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "folder disappeared"))?;
        if node.is_placeholder || node.is_removed || node.is_moved || node.hidden || !node.is_dir {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a folder"));
        }
        self.mount_path(&node.path)
    }

    pub fn mount_path(&mut self, path: &Path) -> io::Result<()> {
        let path = path_to_utf8(path)?;
        let meta = async_fs::block_on(async_fs::metadata(path.as_bytes()))
            .map_err(|err| trueos_fs_err("metadata", err))?;
        if !meta.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mount target is not a folder",
            ));
        }
        self.root = PathBuf::from(path);
        self.center();
        self.reload()
    }

    pub fn find_node_by_path(&self, path: &Path) -> Option<usize> {
        self.nodes
            .iter()
            .find(|node| {
                !node.is_placeholder
                    && !node.is_removed
                    && !node.is_moved
                    && !node.hidden
                    && node.path == path
            })
            .map(|node| node.id)
    }

    fn scan_dir(
        &mut self,
        parent_id: usize,
        dir: &Path,
        depth: usize,
        remaining: &mut usize,
    ) -> io::Result<bool> {
        // depth_limit counts nested levels *after* the immediate children of
        // the mounted directory. Thus depth 0 still renders direct siblings,
        // while depth 4 renders filesystem depths 1 through 5.
        let max_visible_depth = self.depth_limit.saturating_add(1).min(HARD_MAX_DEPTH);
        if depth > max_visible_depth {
            return Ok(false);
        }

        let (entries, listing_truncated) = match self.read_dir_entries(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => return Ok(false),
            Err(err) => return Err(err),
        };

        // Only inspect enough directory entries to establish the per-folder cap.
        // This keeps an enormous directory from becoming an enormous allocation.
        let mut children = Vec::new();
        let mut directory_truncated = listing_truncated;
        for (mut_is_folder, name, path, is_dir, is_symlink, content_type) in entries {
            if children.len() >= MAX_CHILDREN_PER_DIR {
                directory_truncated = true;
                break;
            }

            children.push((mut_is_folder, name, path, is_dir, is_symlink, content_type));
        }

        children.sort_by(|a, b| (a.0, a.1.to_lowercase()).cmp(&(b.0, b.1.to_lowercase())));

        // Add immediate children first. In particular, the mount root gets all of
        // its visible top-level islands before any one subtree can consume the
        // global 256-node rendering budget.
        let mut recurse = Vec::new();
        let mut globally_truncated = false;
        for (_, name, path, is_dir, is_symlink, content_type) in children {
            if *remaining == 0 {
                globally_truncated = true;
                break;
            }

            let id = self.nodes.len();
            self.nodes.push(FsNode {
                id,
                parent: Some(parent_id),
                path: path.clone(),
                name,
                is_dir,
                content_type,
                is_placeholder: false,
                is_removed: false,
                is_moved: false,
                hidden: false,
                depth,
            });
            *remaining -= 1;

            if is_dir && !is_symlink {
                recurse.push((id, path));
            }
        }

        if directory_truncated || globally_truncated {
            self.push_placeholder(parent_id, depth);
        }
        if globally_truncated {
            return Ok(true);
        }

        for (id, path) in recurse {
            if *remaining == 0 {
                self.push_placeholder(parent_id, depth);
                return Ok(true);
            }
            if self.scan_dir(id, &path, depth + 1, remaining)? {
                self.push_placeholder(parent_id, depth);
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn read_dir_entries(
        &self,
        dir: &Path,
    ) -> io::Result<(
        Vec<(
            bool,
            String,
            PathBuf,
            bool,
            bool,
            trueos::content_identity::ContentTypeId,
        )>,
        bool,
    )> {
        let mut entries = Vec::new();
        let path = dir.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "path is not valid UTF-8")
        })?;
        let listing = async_fs::block_on(async_fs::list_dir_typed(path.as_bytes()))
            .map_err(|err| io::Error::other(format!("list_dir failed ({err})")))?;
        for entry in listing.entries {
            if entry.name == ".explorer-trash" {
                continue;
            }
            let is_dir = matches!(entry.kind, async_fs::NodeKind::Directory);
            let child_path = dir.join(entry.name.as_str());
            entries.push((
                !is_dir,
                entry.name,
                child_path,
                is_dir,
                false,
                entry.content_type,
            ));
        }
        Ok((entries, listing.truncated))
    }

    fn push_placeholder(&mut self, parent_id: usize, depth: usize) {
        if self
            .nodes
            .iter()
            .any(|node| node.parent == Some(parent_id) && node.is_placeholder)
        {
            return;
        }

        let id = self.nodes.len();
        self.nodes.push(FsNode {
            id,
            parent: Some(parent_id),
            path: PathBuf::new(),
            name: "...".to_string(),
            is_dir: false,
            content_type: trueos::content_identity::ContentTypeId::NONE,
            is_placeholder: true,
            is_removed: false,
            is_moved: false,
            hidden: false,
            depth,
        });
    }

    fn rebuild_layout(&mut self) {
        let layout_nodes: Vec<LayoutNode> = self
            .nodes
            .iter()
            .map(|node| LayoutNode {
                id: node.id,
                parent: node.parent,
                depth: node.depth,
                visual_width: if node.is_placeholder {
                    3
                } else {
                    // icon + one separating space + filename/folder name
                    2 + node.name.chars().count()
                },
                is_dir: node.is_dir,
            })
            .collect();

        self.positions = match self.layout_mode {
            LayoutMode::Tree => layout::tree_layout(&layout_nodes, self.column_gap),
            LayoutMode::Radial => layout::radial_layout(&layout_nodes, self.column_gap),
        };
        self.suppressed_parent_edges = match self.layout_mode {
            LayoutMode::Tree => layout::tree_suppressed_parent_edges(&layout_nodes),
            LayoutMode::Radial => vec![false; layout_nodes.len()],
        };
        self.rebuild_edge_cache();
        self.bump_scene_revision();
    }

    fn rebuild_edge_cache(&mut self) {
        let mut braille = WorldBrailleCanvas::new();
        for node in &self.nodes {
            if node.hidden {
                continue;
            }
            let Some(parent_id) = node.parent else {
                continue;
            };
            if parent_id == 0
                || self
                    .node(parent_id)
                    .map(|parent| parent.hidden)
                    .unwrap_or(true)
            {
                continue;
            }
            if self.layout_mode == LayoutMode::Tree
                && self
                    .suppressed_parent_edges
                    .get(node.id)
                    .copied()
                    .unwrap_or(false)
            {
                continue;
            }

            let Some(parent_pos) = self.positions.get(parent_id).copied() else {
                continue;
            };
            let Some(child_pos) = self.positions.get(node.id).copied() else {
                continue;
            };
            let parent = (parent_pos.x, parent_pos.y);
            let child = (child_pos.x, child_pos.y);
            let a = edge_anchor(
                parent,
                self.node_width(parent_id),
                child,
                self.node_width(node.id),
            );
            let b = edge_anchor(
                child,
                self.node_width(node.id),
                parent,
                self.node_width(parent_id),
            );

            match self.line_style {
                LineStyle::Default => match self.layout_mode {
                    LayoutMode::Tree => braille.draw_s_curve(a, b),
                    LayoutMode::Radial => braille.draw_radial_spline(a, b),
                },
                LineStyle::Straight => braille.draw_straight(a, b),
                LineStyle::Elbow => braille.draw_elbow(a, b),
                LineStyle::SoftArc => braille.draw_soft_arc(a, b),
            }
        }
        self.edge_cells = braille.into_cells();
    }

    pub fn set_layout_mode(&mut self, mode: LayoutMode) -> bool {
        if self.layout_mode == mode {
            self.center();
            return false;
        }
        self.layout_mode = mode;
        self.rebuild_layout();
        self.center();
        true
    }

    pub fn layout_mode(&self) -> LayoutMode {
        self.layout_mode
    }

    pub fn center(&mut self) {
        self.camera_x = 0;
        self.camera_y = 0;
    }

    pub fn camera(&self) -> (i32, i32) {
        (self.camera_x, self.camera_y)
    }

    /// World-space bounds of everything that can contribute pixels/cells to the
    /// graph: visible labels/tombstones/placeholders plus cached Braille edges.
    /// The hidden structural mount root is deliberately excluded.
    pub fn content_bounds(&self) -> Option<WorldBounds> {
        let mut bounds: Option<WorldBounds> = None;

        let mut include = |x0: i32, x1: i32, y0: i32, y1: i32| {
            bounds = Some(match bounds {
                Some(mut b) => {
                    b.min_x = b.min_x.min(x0);
                    b.max_x = b.max_x.max(x1);
                    b.min_y = b.min_y.min(y0);
                    b.max_y = b.max_y.max(y1);
                    b
                }
                None => WorldBounds {
                    min_x: x0,
                    max_x: x1,
                    min_y: y0,
                    max_y: y1,
                },
            });
        };

        for node in &self.nodes {
            if node.id == 0 || node.hidden {
                continue;
            }
            let Some(pos) = self.positions.get(node.id).copied() else {
                continue;
            };
            let width = self.node_width(node.id).max(1) as i32;
            include(pos.x, pos.x + width - 1, pos.y, pos.y);
        }

        for cell in &self.edge_cells {
            include(cell.x, cell.x, cell.y, cell.y);
        }

        bounds
    }

    /// Camera limits derived from the current content rectangle and viewport.
    ///
    /// The user may deliberately overscroll away from the graph, but only by
    /// half of the currently visible graph viewport on each axis. This keeps
    /// navigation forgiving without allowing the camera to disappear into
    /// effectively unbounded empty space. Camera zero is always valid.
    pub fn camera_limits(&self, viewport: Viewport) -> ((i32, i32), (i32, i32)) {
        let Some(bounds) = self.content_bounds() else {
            return ((0, 0), (0, 0));
        };
        let (origin_x, origin_y) = self.screen_origin(viewport);

        // Baseline: content can reach the viewport edge. Then extend that legal
        // range by exactly half the visible width/height as soft overscroll.
        let left_aligned = viewport.x as i32 - origin_x - bounds.min_x;
        let right_aligned = viewport.right() as i32 - 1 - origin_x - bounds.max_x;
        let top_aligned = viewport.y as i32 - origin_y - bounds.min_y;
        let bottom_aligned = viewport.bottom() as i32 - 1 - origin_y - bounds.max_y;

        let overscroll_x = viewport.width as i32 / 2;
        let overscroll_y = viewport.height as i32 / 2;

        let min_x = right_aligned.saturating_sub(overscroll_x).min(0);
        let max_x = left_aligned.saturating_add(overscroll_x).max(0);
        let min_y = bottom_aligned.saturating_sub(overscroll_y).min(0);
        let max_y = top_aligned.saturating_add(overscroll_y).max(0);
        ((min_x, max_x), (min_y, max_y))
    }

    pub fn set_camera_clamped(&mut self, viewport: Viewport, x: i32, y: i32) -> bool {
        let ((min_x, max_x), (min_y, max_y)) = self.camera_limits(viewport);
        let next_x = x.clamp(min_x, max_x);
        let next_y = y.clamp(min_y, max_y);
        let changed = (next_x, next_y) != (self.camera_x, self.camera_y);
        self.camera_x = next_x;
        self.camera_y = next_y;
        changed
    }

    pub fn clamp_camera(&mut self, viewport: Viewport) -> bool {
        self.set_camera_clamped(viewport, self.camera_x, self.camera_y)
    }

    pub fn pan_clamped(&mut self, viewport: Viewport, dx: i32, dy: i32) -> bool {
        self.set_camera_clamped(
            viewport,
            self.camera_x.saturating_add(dx),
            self.camera_y.saturating_add(dy),
        )
    }

    /// Place a world-space point at the center of the current viewport, then
    /// clamp the result to the same content rectangle used by normal panning.
    pub fn jump_to_world(&mut self, viewport: Viewport, world_x: i32, world_y: i32) -> bool {
        let (origin_x, origin_y) = self.screen_origin(viewport);
        let center_x = viewport.x as i32 + viewport.width as i32 / 2;
        let center_y = viewport.y as i32 + viewport.height as i32 / 2;
        self.set_camera_clamped(
            viewport,
            center_x - origin_x - world_x,
            center_y - origin_y - world_y,
        )
    }

    pub fn line_style(&self) -> LineStyle {
        self.line_style
    }

    pub fn cycle_line_style(&mut self) -> LineStyle {
        self.line_style = self.line_style.next();
        // Connector style is intentionally independent from filesystem scanning
        // and node placement. Only the sparse Braille edge cache changes.
        self.rebuild_edge_cache();
        self.line_style
    }

    pub fn depth_limit(&self) -> usize {
        self.depth_limit
    }

    pub fn set_depth_limit(&mut self, depth: usize) -> io::Result<usize> {
        if !DEPTH_LEVELS.contains(&depth) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "depth must be between 0 and 7",
            ));
        }
        if self.depth_limit != depth {
            self.depth_limit = depth;
            self.reload()?;
        }
        Ok(self.depth_limit)
    }

    pub fn cycle_depth(&mut self) -> io::Result<usize> {
        let current = DEPTH_LEVELS
            .iter()
            .position(|value| *value == self.depth_limit)
            .unwrap_or_else(|| {
                DEPTH_LEVELS
                    .iter()
                    .position(|value| *value == DEFAULT_DEPTH_LIMIT)
                    .unwrap_or(0)
            });
        self.depth_limit = DEPTH_LEVELS[(current + 1) % DEPTH_LEVELS.len()];
        self.reload()?;
        Ok(self.depth_limit)
    }

    pub fn cycle_spacing(&mut self) -> i32 {
        self.column_gap += 4;
        if self.column_gap > 30 {
            self.column_gap = 14;
        }
        self.rebuild_layout();
        self.column_gap
    }

    pub fn spacing_level(&self) -> usize {
        match self.column_gap {
            14 => 0,
            18 => 1,
            22 => 2,
            26 => 3,
            30 => 4,
            value if value < 18 => 0,
            value if value < 22 => 1,
            value if value < 26 => 2,
            value if value < 30 => 3,
            _ => 4,
        }
    }

    pub fn toggle_layout_mode(&mut self) -> LayoutMode {
        let next = match self.layout_mode {
            LayoutMode::Tree => LayoutMode::Radial,
            LayoutMode::Radial => LayoutMode::Tree,
        };
        self.set_layout_mode(next);
        self.layout_mode
    }

    pub fn root_label(&self) -> String {
        self.root.display().to_string()
    }

    pub fn root_path(&self) -> &Path {
        &self.root
    }

    /// Revision for world-space geometry/content used by passive overlays such
    /// as the minimap. Camera movement deliberately does not change it.
    pub fn scene_revision(&self) -> u64 {
        self.scene_revision
    }

    /// Sparse world-space samples for the minimap. This intentionally ignores
    /// the current camera and line-style raster. Nodes plus a few parent-edge
    /// samples are enough to preserve the graph's overall silhouette/density.
    pub fn minimap_samples(&self) -> Vec<(i32, i32)> {
        let mut samples = Vec::with_capacity(self.nodes.len().saturating_mul(4));

        for node in &self.nodes {
            if node.id == 0 || node.hidden || node.is_removed || node.is_moved {
                continue;
            }
            let Some(pos) = self.positions.get(node.id).copied() else {
                continue;
            };
            let center = (pos.x + self.node_width(node.id) as i32 / 2, pos.y);
            samples.push(center);

            let Some(parent_id) = node.parent else {
                continue;
            };
            if parent_id == 0 {
                continue;
            }
            if self.layout_mode == LayoutMode::Tree
                && self
                    .suppressed_parent_edges
                    .get(node.id)
                    .copied()
                    .unwrap_or(false)
            {
                continue;
            }
            let Some(parent) = self.node(parent_id) else {
                continue;
            };
            if parent.hidden || parent.is_removed || parent.is_moved {
                continue;
            }
            let Some(parent_pos) = self.positions.get(parent_id).copied() else {
                continue;
            };
            let parent_center = (
                parent_pos.x + self.node_width(parent_id) as i32 / 2,
                parent_pos.y,
            );

            // Three cheap interpolation samples suggest hierarchy without
            // copying the expensive/high-resolution Braille connector raster.
            for step in 1..=3 {
                let x = parent_center.0 + (center.0 - parent_center.0) * step / 4;
                let y = parent_center.1 + (center.1 - parent_center.1) * step / 4;
                samples.push((x, y));
            }
        }

        samples
    }

    fn bump_scene_revision(&mut self) {
        self.scene_revision = self.scene_revision.wrapping_add(1);
    }

    pub fn selection_stats(&self, id: usize) -> Option<SelectionStats> {
        let node = self.node(id)?;
        if node.id == 0 || node.is_placeholder || node.is_removed || node.is_moved || node.hidden {
            return None;
        }

        let metadata = match path_to_utf8(&node.path) {
            Ok(node_path) => {
                async_fs::block_on(async_fs::typed_metadata(node_path.as_bytes())).ok()
            }
            Err(_) => None,
        };

        let size = metadata
            .as_ref()
            .map(|meta| human_bytes(meta.len))
            .unwrap_or_else(|| "?".to_string());
        let modified = "?".to_string();
        let access = "?".to_string();

        Some(SelectionStats {
            key: format!("#{}", node.id),
            kind: if node.is_dir { "folder" } else { "file" }.to_string(),
            size,
            modified,
            access,
            content_type: metadata
                .as_ref()
                .map(|meta| format_content_type(meta.content_type))
                .unwrap_or_else(|| ContentTypeDisplay {
                    raw_hex: "?".to_string(),
                    decimal: "?".to_string(),
                    canonical_name: "unavailable".to_string(),
                    mime: "?".to_string(),
                    status: "unknown".to_string(),
                }),
        })
    }

    pub fn preview_infer(&self, source: usize) -> io::Result<Option<(PathBuf, Vec<u8>, async_fs::ContentTypeId, [u8; 32])>> {
        let node = self.node(source).ok_or_else(|| io::Error::other("file disappeared"))?;
        if node.is_dir || node.is_placeholder || node.is_removed || node.is_moved
            || node.content_type != async_fs::ContentTypeId::BLOB {
            return Ok(None);
        }
        let path = path_to_utf8(&node.path)?;
        let metadata = async_fs::block_on(async_fs::typed_metadata(path.as_bytes()))
            .map_err(|err| trueos_fs_err("infer stat", err))?;
        // Keep a confirmation snapshot bounded; larger files stay untouched.
        if metadata.content_type != async_fs::ContentTypeId::BLOB || metadata.len > 16 * 1024 * 1024 {
            return Err(io::Error::other("infer requires a Blob file up to 16 MiB"));
        }
        let bytes = read_file_bytes(&node.path)?;
        let Some(content_type) = async_fs::infer_named_bytes(path, &bytes) else { return Ok(None); };
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        Ok(Some((node.path.clone(), bytes, content_type, digest)))
    }

    pub fn apply_infer(&mut self, path: &Path, bytes: &[u8], content_type: async_fs::ContentTypeId, digest: [u8; 32]) -> io::Result<()> {
        async_fs::block_on(async_fs::infer_blob_checked(path_to_utf8(path)?.as_bytes(), bytes, content_type, digest))
            .map_err(|err| trueos_fs_err("infer (file may have changed)", err))?;
        self.reload()
    }

    pub fn node(&self, id: usize) -> Option<&FsNode> {
        self.nodes.get(id)
    }

    pub fn video_path_for_node(&self, id: usize) -> io::Result<String> {
        let node = self.node(id).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "selected file no longer exists")
        })?;
        if node.is_dir
            || node.is_placeholder
            || node.is_removed
            || node.is_moved
            || node.hidden
            || !is_playable_video_type(node.content_type)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "select one inferred MP4 or MKV file",
            ));
        }
        let path = path_to_utf8(&node.path)?;
        if path.starts_with("trueosfs:disc") {
            return Ok(path.to_string());
        }
        if !path.starts_with('/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "video path is outside TRUEOSFS",
            ));
        }
        let mounts = async_fs::block_on(async_fs::list_mounts())
            .map_err(|err| trueos_fs_err("list_mounts", err))?;
        let primary = mounts.iter().find(|mount| mount.primary).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "primary TRUEOSFS disc unavailable")
        })?;
        Ok(format!("{}{}", primary.selector, path))
    }

    pub fn image_paths_for_selection(
        &self,
        selected: Option<usize>,
        selected_files: &[usize],
    ) -> io::Result<Vec<String>> {
        let ids: Vec<usize> = if selected_files.is_empty() {
            selected.into_iter().collect()
        } else {
            selected_files.to_vec()
        };
        if ids.is_empty() || ids.len() > 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "select 1..=32 images or one image folder",
            ));
        }

        if ids.len() == 1 {
            let node = self.node(ids[0]).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "selected item no longer exists")
            })?;
            if node.is_dir {
                let path = path_to_utf8(&node.path)?;
                let listing = async_fs::block_on(async_fs::list_dir_typed(path.as_bytes()))
                    .map_err(|error| io::Error::other(format!("list_dir failed ({error})")))?;
                let contains_image = listing.entries.iter().any(|entry| {
                    entry.kind == async_fs::NodeKind::File
                        && is_viewable_image_type(entry.content_type)
                });
                return if contains_image {
                    Ok(vec![path.to_string()])
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "folder contains no inferred PNG/JPEG images",
                    ))
                };
            }
        }

        ids.into_iter()
            .map(|id| {
                let node = self.node(id).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "selected file no longer exists")
                })?;
                if node.is_dir || !is_viewable_image_type(node.content_type) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "selection contains a non-image file",
                    ));
                }
                path_to_utf8(&node.path).map(str::to_string)
            })
            .collect()
    }

    pub fn sha256_node(&self, id: usize) -> io::Result<Sha256Result> {
        let node = self.node(id).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "selected file no longer exists")
        })?;
        if node.is_placeholder || node.is_removed || node.is_moved || node.hidden || node.is_dir {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "select one regular file",
            ));
        }

        let digest = sha256_hex(&read_file_bytes(&node.path)?);
        Ok(Sha256Result {
            source: id,
            digest,
            path: node.path.clone(),
        })
    }

    pub fn archive_default_folder(&self, id: usize) -> Option<String> {
        let node = self.node(id)?;
        if node.is_dir || node.is_placeholder || node.is_removed || node.is_moved || node.hidden {
            return None;
        }
        let suffix = archive_suffix(&node.name)?;
        Some(node.name[..node.name.len() - suffix.len()].to_owned())
    }

    pub fn extract_archive_node(&mut self, id: usize, folder: &str) -> io::Result<String> {
        validate_name(folder)?;
        if self.archive_default_folder(id).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "select an archive file",
            ));
        }
        self.archive_node_with_folder(id, Some(folder.trim()))
    }

    pub fn archive_node(&mut self, id: usize) -> io::Result<String> {
        self.archive_node_with_folder(id, None)
    }

    fn archive_node_with_folder(&mut self, id: usize, folder: Option<&str>) -> io::Result<String> {
        crate::archive_log(format_args!("termdir/archive: phase=prepare node={id} folder={folder:?} busy={}", !self.pending_archives.is_empty()));
        let is_unpack = self.archive_default_folder(id).is_some();
        if !self.pending_archives.is_empty() && !is_unpack {
            return Ok("ARCHIVE · an operation is already running".into());
        }
        let node = self.node(id).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "selected item no longer exists")
        })?;
        if node.is_placeholder || node.is_removed || node.is_moved || node.hidden {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "select one file or folder",
            ));
        }

        let source = node.path.clone();
        if self.pending_archives.iter().any(|job| job.source.as_ref() == Some(&source)) {
            return Ok("ARCHIVE · this archive is already unpacking".into());
        }
        crate::archive_log(format_args!("termdir/archive: phase=source path={:?} directory={}", source, node.is_dir));
        let source_text = path_to_utf8(&source)?;
        let suffix = if node.is_dir {
            None
        } else {
            archive_suffix(source_text)
        };
        let extracting = suffix.is_some();
        let label = "LZ4";
        let preferred = match suffix {
            Some(suffix) => PathBuf::from(&source_text[..source_text.len() - suffix.len()]),
            None => PathBuf::from(format!("{}.tar.lz4", source.display())),
        };
        if preferred.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "archive path has no output name",
            ));
        }
        let destination = if let Some(folder) = folder {
            let destination = source
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(folder);
            if trueos_exists(&destination)? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "destination folder already exists",
                ));
            }
            destination
        } else {
            unique_archive_destination(&preferred, !extracting)?
        };

        let unpack_source = extracting.then(|| source.clone());
        let source = path_to_utf8(&source)?.to_owned();
        let destination = path_to_utf8(&destination)?.to_owned();
        if self.pending_archives.iter().any(|job| {
            let active = Path::new(&job.destination);
            let requested = Path::new(&destination);
            active.starts_with(requested) || requested.starts_with(active)
        }) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "destination is already being written by an archive operation"));
        }
        let progress = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed_progress = progress.clone();
        let target = destination.clone();
        crate::archive_log(format_args!("termdir/archive: phase=future-created extracting={extracting} source={source:?} destination={destination:?}"));
        self.pending_archives.push(PendingArchive {
            future: Box::pin(async move {
                crate::archive_log(format_args!("termdir/archive: phase=api-call extracting={extracting} source={source:?} destination={target:?}"));
                if extracting {
                    trueos::archive::unpack_with_progress(source.as_bytes(), target.as_bytes(), move |percent| observed_progress.set(percent)).await
                } else {
                    trueos::archive::pack(source.as_bytes(), target.as_bytes()).await
                }
            }),
            destination: destination.clone(),
            label,
            last_marker: std::time::Instant::now(),
            source: unpack_source,
            progress,
            started: std::time::Instant::now(),
            next_poll: std::time::Instant::now(),
            displayed_percent: Some(0),
        });
        self.bump_scene_revision();
        Ok(format!("{label} · running · {destination}"))
    }

    /// Observe all jobs on the same cadence as the falling drop animation.
    pub fn poll_archive(&mut self, now: std::time::Instant) -> Option<String> {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let mut completed = Vec::new();
        let mut changed = false;
        let mut refresh = false;
        self.pending_archives.retain_mut(|pending| {
            if now < pending.next_poll { return true; }
            pending.next_poll = now + crate::FALL_TICK;
            if now.duration_since(pending.last_marker) >= std::time::Duration::from_secs(5) {
                crate::archive_log(format_args!("termdir/archive: phase=ui-wait destination={:?}", pending.destination));
                pending.last_marker = now;
            }
            let result = pending.future.as_mut().poll(&mut context);
            let percent = pending.source.as_ref().and_then(|_| unpack_label_percent(
                now.saturating_duration_since(pending.started), pending.progress.get()));
            changed |= percent != pending.displayed_percent;
            pending.displayed_percent = percent;
            let std::task::Poll::Ready(result) = result else { return true; };
            changed = true;
            crate::archive_log(format_args!("termdir/archive: phase=api-return destination={:?} result={result:?}", pending.destination));
            completed.push(match result {
                Ok(report) => {
                    refresh = true;
                    format!("{} · {} · {} file(s) · {} → {} bytes", pending.label,
                        pending.destination, report.file_count, report.input_bytes, report.output_bytes)
                }
                Err(error) => format!("ARCHIVE FAILED · {}", trueos_fs_err("archive", error)),
            });
            false
        });
        if changed { self.bump_scene_revision(); }
        if refresh {
            if let Err(error) = self.reload() {
                completed.push(format!("refresh failed: {error}"));
            }
        }
        (!completed.is_empty()).then(|| completed.join(" · "))
    }

    /// Bundle explicit regular files from one directory. A multi-file archive
    /// intentionally uses the basename of every file as its archive entry;
    /// the explorer only creates this selection inside one parent directory,
    /// so every entry name is unique and extraction restores those siblings.
    pub fn archive_nodes(&mut self, sources: &[usize]) -> io::Result<String> {
        if !self.pending_archives.is_empty() {
            return Ok("ARCHIVE · an operation is already running".into());
        }
        if sources.len() == 1 {
            return self.archive_node(sources[0]);
        }
        let nodes = self.selected_regular_files(sources)?;
        let parent = nodes[0].path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "selected file has no parent")
        })?;
        let destination = unique_archive_destination(&parent.join("bundle.tar.lz4"), true)?;
        let source_paths: Vec<String> = nodes
            .iter()
            .map(|node| path_to_utf8(&node.path).map(str::to_owned))
            .collect::<io::Result<_>>()?;
        let destination = path_to_utf8(&destination)?.to_owned();
        let target = destination.clone();
        self.pending_archives.push(PendingArchive {
            future: Box::pin(async move {
                crate::archive_log(format_args!("termdir/archive: phase=api-call pack-many sources={source_paths:?} destination={target:?}"));
                let paths: Vec<_> = source_paths.iter().map(|path| path.as_bytes()).collect();
                trueos::archive::pack_many(&paths, target.as_bytes()).await
            }),
            destination: destination.clone(),
            label: "LZ4",
            last_marker: std::time::Instant::now(),
            source: None,
            progress: std::rc::Rc::new(std::cell::Cell::new(0)),
            started: std::time::Instant::now(),
            next_poll: std::time::Instant::now(),
            displayed_percent: None,
        });
        Ok(format!("LZ4 · running · {destination}"))
    }

    pub fn label(&self, id: usize) -> String {
        self.node(id)
            .map(|n| {
                if n.is_placeholder {
                    "...".to_string()
                } else if n.is_dir {
                    format!("{}/", n.name.trim_end_matches('/'))
                } else {
                    n.name.clone()
                }
            })
            .unwrap_or_else(|| "?".to_string())
    }

    fn visual_label(&self, id: usize) -> String {
        self.node(id)
            .map(|n| {
                if n.hidden {
                    String::new()
                } else if n.is_removed {
                    "🪦 removed".to_string()
                } else if n.is_moved {
                    "moved".to_string()
                } else if n.is_placeholder {
                    "...".to_string()
                } else {
                    let icon = if n.is_dir { "🖿" } else { "🖹" };
                    let progress = self.pending_archives.iter()
                        .find(|job| job.source.as_ref() == Some(&n.path))
                        .and_then(|job| job.displayed_percent);
                    match progress {
                        Some(percent) => format!("{icon} {percent}%"),
                        None => format!("{icon} {}", n.name),
                    }
                }
            })
            .unwrap_or_else(|| "?".to_string())
    }

    pub fn hit_test(&self, viewport: Viewport, x: u16, y: u16) -> Option<usize> {
        if !viewport.contains(x, y) {
            return None;
        }

        for node in self.nodes.iter().rev() {
            if node.id == 0
                || node.is_placeholder
                || node.is_removed
                || node.is_moved
                || node.hidden
            {
                continue;
            }
            let Some((sx, sy)) = self.screen_pos(node.id, viewport) else {
                continue;
            };
            let width = self.node_width(node.id) as i32;
            if y as i32 == sy && x as i32 >= sx && (x as i32) < sx + width {
                return Some(node.id);
            }
        }
        None
    }

    pub fn folder_drop_target(
        &self,
        viewport: Viewport,
        x: u16,
        y: u16,
        source: usize,
    ) -> Option<usize> {
        self.folder_drop_target_many(viewport, x, y, &[source])
    }

    pub fn folder_drop_target_many(
        &self,
        viewport: Viewport,
        x: u16,
        y: u16,
        sources: &[usize],
    ) -> Option<usize> {
        if !viewport.contains(x, y) || sources.is_empty() {
            return None;
        }

        // Folder labels remain visually unchanged, but their drop target grows
        // by one terminal cell on every side. The tree layout reserves extra Y
        // room between folder siblings so these virtual hit areas stay usable.
        for node in self.nodes.iter().rev() {
            if node.id == 0
                || node.is_placeholder
                || node.is_removed
                || node.hidden
                || !node.is_dir
                || !sources.iter().all(|source| self.can_move(*source, node.id))
            {
                continue;
            }
            let Some((sx, sy)) = self.screen_pos(node.id, viewport) else {
                continue;
            };
            let width = self.node_width(node.id) as i32;
            let px = x as i32;
            let py = y as i32;
            if px >= sx - 1 && px <= sx + width && py >= sy - 1 && py <= sy + 1 {
                return Some(node.id);
            }
        }
        None
    }

    pub fn can_move(&self, source: usize, target: usize) -> bool {
        if source == 0 || source == target {
            return false;
        }
        let Some(src) = self.node(source) else {
            return false;
        };
        let Some(dst) = self.node(target) else {
            return false;
        };
        if src.is_placeholder
            || dst.is_placeholder
            || src.is_removed
            || src.is_moved
            || dst.is_removed
            || dst.is_moved
            || src.hidden
            || dst.hidden
            || !dst.is_dir
            || src.parent == Some(target)
        {
            return false;
        }
        if src.is_dir && self.is_descendant(target, source) {
            return false;
        }
        true
    }

    pub fn can_move_to_path(&self, source: usize, target: &Path) -> bool {
        if source == 0 {
            return false;
        }
        let Some(src) = self.node(source) else {
            return false;
        };
        if src.is_placeholder
            || src.is_removed
            || src.is_moved
            || src.hidden
            || src.path.parent() == Some(target)
        {
            return false;
        }
        if target == src.path || (src.is_dir && target.starts_with(&src.path)) {
            return false;
        }
        true
    }

    pub fn can_move_path(&self, source: &Path, target: &Path) -> bool {
        source.file_name().is_some()
            && source.parent() != Some(target)
            && source != target
            && !target.starts_with(source)
    }

    fn is_descendant(&self, candidate: usize, ancestor: usize) -> bool {
        let mut current = self.node(candidate).and_then(|n| n.parent);
        while let Some(id) = current {
            if id == ancestor {
                return true;
            }
            current = self.node(id).and_then(|n| n.parent);
        }
        false
    }

    pub fn create_folder(&mut self, parent: usize, name: &str) -> io::Result<String> {
        validate_name(name)?;
        let parent_path = if parent == 0 {
            self.root.clone()
        } else {
            let node = self.node(parent).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "parent folder disappeared")
            })?;
            if node.is_placeholder || !node.is_dir {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "parent is not a folder",
                ));
            }
            node.path.clone()
        };
        let destination = parent_path.join(name);
        if trueos_exists(&destination)? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "name already exists",
            ));
        }
        async_fs::block_on(async_fs::create_dir_all(
            path_to_utf8(&destination)?.as_bytes(),
        ))
        .map_err(|err| trueos_fs_err("create_dir_all", err))?;
        let message = format!("NEW · 🖿 {name}");
        self.reload()?;
        Ok(message)
    }

    pub fn create_file(&mut self, parent: usize, name: &str) -> io::Result<String> {
        validate_name(name)?;
        let parent_path = if parent == 0 {
            self.root.clone()
        } else {
            let node = self.node(parent).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "parent folder disappeared")
            })?;
            if node.is_placeholder
                || node.is_removed
                || node.is_moved
                || node.hidden
                || !node.is_dir
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "parent is not a folder",
                ));
            }
            node.path.clone()
        };
        let destination = parent_path.join(name);
        if trueos_exists(&destination)? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "name already exists",
            ));
        }
        async_fs::block_on(async_fs::write_file(
            path_to_utf8(&destination)?.as_bytes(),
            b"",
        ))
        .map_err(|err| trueos_fs_err("write_file", err))?;
        let message = format!("NEW · 🖹 {name}");
        self.reload()?;
        Ok(message)
    }

    pub fn rename_node(&mut self, source: usize, name: &str) -> io::Result<String> {
        validate_name(name)?;
        if source == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot rename mount root",
            ));
        }
        let node = self
            .node(source)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "source node disappeared"))?;
        if node.is_placeholder {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot rename placeholder",
            ));
        }
        let parent = node
            .path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "source has no parent"))?;
        let destination = parent.join(name);
        if destination == node.path {
            return Ok(format!("NAME · {}", node.name));
        }
        if trueos_exists(&destination)? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "name already exists",
            ));
        }
        trueos_rename(&node.path, &destination)?;
        let message = format!("NAME · {} → {name}", node.name);
        self.reload()?;
        Ok(message)
    }

    pub fn move_node(&mut self, source: usize, target: usize) -> io::Result<String> {
        if !self.can_move(source, target) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid folder move",
            ));
        }

        let src = self
            .node(source)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "source node disappeared"))?;
        let dst = self
            .node(target)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "target folder disappeared"))?;
        let name = src.path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "source has no file name")
        })?;
        let destination = dst.path.join(name);
        if trueos_exists(&destination)? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists in {}", src.name, dst.name),
            ));
        }

        trueos_rename(&src.path, &destination)?;
        self.mark_moved(source);
        let message = format!("MOVE · {} → {}", src.name, dst.name);
        Ok(message)
    }

    /// Move every selected regular file after checking all destinations before
    /// the first rename. The filesystem currently exposes individual renames,
    /// so this preflight is the strongest atomicity boundary available here.
    pub fn move_nodes(&mut self, sources: &[usize], target: usize) -> io::Result<String> {
        let nodes = self.selected_regular_files(sources)?;
        if !sources.iter().all(|source| self.can_move(*source, target)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid file-group move",
            ));
        }
        let target_node = self
            .node(target)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "target folder disappeared"))?;
        let mut destination_set = BTreeSet::new();
        let mut destinations = Vec::new();
        destinations
            .try_reserve_exact(nodes.len())
            .map_err(|_| io::Error::other("move allocation failed"))?;
        for source in &nodes {
            let name = source.path.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "source has no file name")
            })?;
            let destination = target_node.path.join(name);
            if !destination_set.insert(destination.clone()) || trueos_exists(&destination)? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} already exists in {}", source.name, target_node.name),
                ));
            }
            destinations.push(destination);
        }
        for (source, destination) in nodes.iter().zip(destinations.iter()) {
            trueos_rename(&source.path, destination)?;
        }
        self.mark_moved_nodes(sources);
        Ok(format!(
            "MOVE · {} files → {}",
            sources.len(),
            target_node.name
        ))
    }

    pub fn move_node_to_path(&mut self, source: usize, target: &Path) -> io::Result<String> {
        if !self.can_move_to_path(source, target) || !trueos_is_dir(target)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid folder move",
            ));
        }

        let src = self
            .node(source)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "source node disappeared"))?;
        let name = src.path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "source has no file name")
        })?;
        let destination = target.join(name);
        if trueos_exists(&destination)? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists in {}", src.name, target.display()),
            ));
        }

        trueos_rename(&src.path, &destination)?;
        self.mark_moved(source);
        let message = format!("MOVE · {} → {}", src.name, target.display());
        Ok(message)
    }

    pub fn move_nodes_to_path(&mut self, sources: &[usize], target: &Path) -> io::Result<String> {
        let nodes = self.selected_regular_files(sources)?;
        if !sources
            .iter()
            .all(|source| self.can_move_to_path(*source, target))
            || !trueos_is_dir(target)?
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid file-group move",
            ));
        }
        let mut destination_set = BTreeSet::new();
        let mut destinations = Vec::new();
        destinations
            .try_reserve_exact(nodes.len())
            .map_err(|_| io::Error::other("move allocation failed"))?;
        for source in &nodes {
            let name = source.path.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "source has no file name")
            })?;
            let destination = target.join(name);
            if !destination_set.insert(destination.clone()) || trueos_exists(&destination)? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} already exists in {}", source.name, target.display()),
                ));
            }
            destinations.push(destination);
        }
        for (source, destination) in nodes.iter().zip(destinations.iter()) {
            trueos_rename(&source.path, destination)?;
        }
        self.mark_moved_nodes(sources);
        Ok(format!(
            "MOVE · {} files → {}",
            sources.len(),
            target.display()
        ))
    }

    pub fn move_path(&mut self, source: &Path, target: &Path) -> io::Result<String> {
        if !self.can_move_path(source, target) || !trueos_is_dir(target)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid folder move",
            ));
        }
        if !trueos_exists(source)? {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "linked source disappeared",
            ));
        }

        let name = source.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "source has no file name")
        })?;
        let destination = target.join(name);
        if trueos_exists(&destination)? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{} already exists in {}",
                    name.to_string_lossy(),
                    target.display()
                ),
            ));
        }

        trueos_rename(source, &destination)?;
        if let Some(source_id) = self.find_node_by_path(source) {
            self.mark_moved(source_id);
        }
        Ok(format!(
            "MOVE · {} → {}",
            name.to_string_lossy(),
            target.display()
        ))
    }

    pub fn move_paths(&mut self, sources: &[PathBuf], target: &Path) -> io::Result<String> {
        if sources.is_empty() || !trueos_is_dir(target)? {
            return Err(io::Error::other("invalid folder move"));
        }
        // Validate every destination before moving any member of the payload.
        let mut destinations = BTreeSet::new();
        for source in sources {
            if !self.can_move_path(source, target) || !trueos_exists(source)? {
                return Err(io::Error::other("invalid or missing drag source"));
            }
            let name = source
                .file_name()
                .ok_or_else(|| io::Error::other("source has no file name"))?;
            let destination = target.join(name);
            if !destinations.insert(destination.clone()) || trueos_exists(&destination)? {
                return Err(io::Error::other("drag destination already exists"));
            }
        }
        for source in sources {
            self.move_path(source, target)?;
        }
        Ok(format!("MOVE · {} entities → {}", sources.len(), target.display()))
    }

    pub fn trash_node(&mut self, source: usize) -> io::Result<String> {
        if source == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot trash root",
            ));
        }

        let src = self
            .node(source)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "source node disappeared"))?;
        if src.is_placeholder {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot trash placeholder",
            ));
        }

        let trash = self.root.join(".explorer-trash");
        async_fs::block_on(async_fs::create_dir_all(path_to_utf8(&trash)?.as_bytes()))
            .map_err(|err| trueos_fs_err("create_dir_all", err))?;

        let original = src.path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "source has no file name")
        })?;
        let mut destination = trash.join(original);

        if trueos_exists(&destination)? {
            let stem = src
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("item");
            let ext = src.path.extension().and_then(|s| s.to_str());
            for n in 2..10_000 {
                let candidate = match ext {
                    Some(ext) if !src.is_dir => format!("{stem}.{n}.{ext}"),
                    _ => format!("{stem}.{n}"),
                };
                destination = trash.join(candidate);
                if !trueos_exists(&destination)? {
                    break;
                }
            }
        }

        trueos_rename(&src.path, &destination)?;

        // Keep the current world layout stable after deletion. The deleted node
        // remains as a non-interactive tombstone until the next explicit reload,
        // mount/parent change, or depth refresh. Descendants of a removed folder
        // are hidden immediately because they no longer exist at their old paths.
        if let Some(node) = self.nodes.get_mut(source) {
            node.is_removed = true;
        }
        self.hide_descendants(source);

        // Positions are intentionally untouched. Only connector geometry changes:
        // edges inside a removed folder vanish, while its parent→tombstone edge may
        // remain. This is much cheaper than rescanning and laying out the tree.
        self.rebuild_edge_cache();
        self.bump_scene_revision();

        let message = format!("TRASH · {} → .explorer-trash/", src.name);
        Ok(message)
    }

    /// Recycle a file selection as one preflighted group. The existing
    /// single-node delete continues to support folders; this batch operation
    /// is deliberately file-only to match the explorer's selection contract.
    pub fn trash_nodes(&mut self, sources: &[usize]) -> io::Result<String> {
        let nodes = self.selected_regular_files(sources)?;
        let trash = self.root.join(".explorer-trash");
        async_fs::block_on(async_fs::create_dir_all(path_to_utf8(&trash)?.as_bytes()))
            .map_err(|err| trueos_fs_err("create_dir_all", err))?;

        let mut reserved = BTreeSet::new();
        let mut destinations = Vec::new();
        for source in &nodes {
            let destination = self.next_trash_destination(source, &trash, &reserved)?;
            reserved.insert(destination.clone());
            destinations.push(destination);
        }
        for (source, destination) in nodes.iter().zip(destinations.iter()) {
            trueos_rename(&source.path, destination)?;
        }
        self.mark_removed_nodes(sources);
        Ok(format!(
            "TRASH · {} files → .explorer-trash/",
            sources.len()
        ))
    }

    // Keep the old node in its existing slot as a lightweight move marker until
    // the next explicit reload. This mirrors delete's retained tombstone update:
    // no directory scan or layout pass is needed, only the affected subtree and
    // connector cache change.
    fn mark_moved(&mut self, source: usize) {
        if let Some(node) = self.nodes.get_mut(source) {
            node.is_moved = true;
        }
        self.hide_descendants(source);
        self.rebuild_edge_cache();
        self.bump_scene_revision();
    }

    fn mark_moved_nodes(&mut self, sources: &[usize]) {
        for source in sources {
            if let Some(node) = self.nodes.get_mut(*source) {
                node.is_moved = true;
            }
            self.hide_descendants(*source);
        }
        self.rebuild_edge_cache();
        self.bump_scene_revision();
    }

    fn mark_removed_nodes(&mut self, sources: &[usize]) {
        for source in sources {
            if let Some(node) = self.nodes.get_mut(*source) {
                node.is_removed = true;
            }
            self.hide_descendants(*source);
        }
        self.rebuild_edge_cache();
        self.bump_scene_revision();
    }

    fn selected_regular_files(&self, sources: &[usize]) -> io::Result<Vec<FsNode>> {
        if sources.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "select one or more files",
            ));
        }
        let mut ids = BTreeSet::new();
        let mut parent = None;
        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(sources.len())
            .map_err(|_| io::Error::other("selection allocation failed"))?;
        for source in sources {
            if !ids.insert(*source) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "selection contains the same file twice",
                ));
            }
            let node = self.node(*source).cloned().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "selected file disappeared")
            })?;
            if node.is_placeholder || node.is_removed || node.is_moved || node.hidden || node.is_dir
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "select regular files from one folder",
                ));
            }
            let node_parent = node.path.parent().map(Path::to_path_buf).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "selected file has no parent")
            })?;
            if parent.as_ref().is_some_and(|parent| parent != &node_parent) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "selected files must share one folder",
                ));
            }
            parent = Some(node_parent);
            nodes.push(node);
        }
        Ok(nodes)
    }

    fn next_trash_destination(
        &self,
        source: &FsNode,
        trash: &Path,
        reserved: &BTreeSet<PathBuf>,
    ) -> io::Result<PathBuf> {
        let original = source.path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "source has no file name")
        })?;
        for n in 1..10_000 {
            let candidate = if n == 1 {
                trash.join(original)
            } else {
                let stem = source
                    .path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("item");
                let ext = source
                    .path
                    .extension()
                    .and_then(|extension| extension.to_str());
                let name = match ext {
                    Some(ext) if !source.is_dir => format!("{stem}.{n}.{ext}"),
                    _ => format!("{stem}.{n}"),
                };
                trash.join(name)
            };
            if !reserved.contains(&candidate) && !trueos_exists(&candidate)? {
                return Ok(candidate);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no available recycle name",
        ))
    }

    fn hide_descendants(&mut self, ancestor: usize) {
        let mut stack = vec![ancestor];
        while let Some(parent) = stack.pop() {
            let children: Vec<usize> = self
                .nodes
                .iter()
                .filter(|node| node.parent == Some(parent))
                .map(|node| node.id)
                .collect();
            for child in children {
                if let Some(node) = self.nodes.get_mut(child) {
                    node.hidden = true;
                }
                stack.push(child);
            }
        }
    }

    pub fn render(
        &self,
        frame: &mut Frame,
        viewport: Viewport,
        selected: Option<usize>,
        drag_id: Option<usize>,
        drop_target: Option<usize>,
    ) {
        let drag_sources = drag_id.as_slice();
        self.render_many(frame, viewport, selected, &[], None, drag_sources, drop_target);
    }

    pub fn render_many(
        &self,
        frame: &mut Frame,
        viewport: Viewport,
        selected: Option<usize>,
        selected_files: &[usize],
        hovered: Option<usize>,
        drag_sources: &[usize],
        drop_target: Option<usize>,
    ) {
        // Connector geometry is cached in world space whenever the directory,
        // layout mode, or spacing changes. Panning and resize only translate and
        // clip these cached Braille cells.
        self.render_edges(frame, viewport);

        for node in &self.nodes {
            if node.id == 0 || node.hidden {
                continue;
            }
            let Some((sx, sy)) = self.screen_pos(node.id, viewport) else {
                continue;
            };
            if sy < viewport.y as i32 || sy >= viewport.bottom() as i32 {
                continue;
            }

            let label = self.visual_label(node.id);
            let mut style = Style::new(Color::Grey, Color::Reset);

            if node.is_removed || node.is_moved {
                style = Style::new(Color::DarkGrey, Color::Reset);
            } else if node.is_placeholder {
                // Truncation must be visually loud enough to read as an actual
                // node in both tree and radial views.
                style = Style::new(Color::Black, Color::White);
            } else if node.is_dir {
                style = Style::new(Color::Black, Color::White);
            }
            if !node.is_removed && !node.is_moved && hovered == Some(node.id) {
                style = Style::new(Color::Black, Color::Grey);
            }
            if !node.is_removed
                && !node.is_moved
                && (selected == Some(node.id) || selected_files.contains(&node.id))
            {
                style = Style::new(Color::Black, Color::DarkYellow);
            }
            if !node.is_removed && !node.is_moved && drop_target == Some(node.id) {
                style = Style::new(Color::Black, Color::Cyan);
            }
            if !node.is_removed && !node.is_moved && drag_sources.contains(&node.id) {
                style = Style::new(Color::DarkGrey, Color::Reset);
            }

            self.print_clipped(frame, viewport, sx, sy, &label, style);
        }
    }

    fn render_edges(&self, frame: &mut Frame, viewport: Viewport) {
        let (origin_x, origin_y) = self.screen_origin(viewport);
        let offset_x = origin_x + self.camera_x;
        let offset_y = origin_y + self.camera_y;
        let style = Style::new(Color::DarkGrey, Color::Reset);

        // edge_cells is sorted by (world_y, world_x). Convert the visible screen
        // band back to world rows and binary-search directly into the cache. A pan
        // therefore walks only connector cells that can actually reach the current
        // viewport instead of rescanning the complete graph.
        let world_top = viewport.y as i32 - offset_y;
        let world_bottom = viewport.bottom().saturating_sub(1) as i32 - offset_y;
        let first = self.edge_cells.partition_point(|cell| cell.y < world_top);

        for cell in &self.edge_cells[first..] {
            if cell.y > world_bottom {
                break;
            }
            let screen_x = offset_x + cell.x;
            if screen_x < viewport.x as i32 || screen_x >= viewport.right() as i32 {
                continue;
            }
            let screen_y = offset_y + cell.y;
            let ch = char::from_u32(0x2800 + cell.mask as u32).unwrap_or(' ');
            frame.put_i32(screen_x, screen_y, ch, style);
        }
    }

    pub fn screen_y(&self, id: usize, viewport: Viewport) -> Option<u16> {
        let (_, y) = self.screen_pos(id, viewport)?;
        if y < viewport.y as i32 || y >= viewport.bottom() as i32 {
            return None;
        }
        Some(y as u16)
    }

    fn screen_pos(&self, id: usize, viewport: Viewport) -> Option<(i32, i32)> {
        let p = *self.positions.get(id)?;
        let (origin_x, origin_y) = self.screen_origin(viewport);
        Some((
            origin_x + p.x + self.camera_x,
            origin_y + p.y + self.camera_y,
        ))
    }

    fn screen_origin(&self, viewport: Viewport) -> (i32, i32) {
        match self.layout_mode {
            LayoutMode::Tree => (
                viewport.x as i32 + 2,
                viewport.y as i32 + (viewport.height as i32 / 2),
            ),
            LayoutMode::Radial => (
                viewport.x as i32 + (viewport.width as i32 / 2),
                viewport.y as i32 + (viewport.height as i32 / 2),
            ),
        }
    }

    fn node_width(&self, id: usize) -> usize {
        self.node(id)
            .map(|node| {
                if node.hidden {
                    0
                } else if node.is_removed {
                    text_cell_width("🪦 removed")
                } else if node.is_moved {
                    text_cell_width("moved")
                } else if node.is_placeholder {
                    3
                } else {
                    2 + node.name.chars().count()
                }
            })
            .unwrap_or(1)
            .max(1)
    }

    fn print_clipped(
        &self,
        frame: &mut Frame,
        viewport: Viewport,
        x: i32,
        y: i32,
        text: &str,
        style: Style,
    ) {
        if y < viewport.y as i32 || y >= viewport.bottom() as i32 {
            return;
        }

        let mut screen_x = x;
        for ch in text.chars() {
            let glyph_width = terminal_cell_width(ch) as i32;
            let glyph_right = screen_x + glyph_width;

            // Do not paint half of a wide glyph at the viewport boundary.
            if screen_x >= viewport.x as i32 && glyph_right <= viewport.right() as i32 {
                frame.put_display_i32(screen_x, y, ch, style);
            }

            screen_x = glyph_right;
            if screen_x >= viewport.right() as i32 {
                break;
            }
        }
    }
}

pub fn is_playable_video_type(content_type: trueos::content_identity::ContentTypeId) -> bool {
    matches!(
        content_type,
        trueos::content_identity::ContentTypeId::MP4
            | trueos::content_identity::ContentTypeId::MATROSKA
    )
}

fn is_viewable_image_type(content_type: trueos::content_identity::ContentTypeId) -> bool {
    matches!(
        content_type,
        trueos::content_identity::ContentTypeId::JPEG
            | trueos::content_identity::ContentTypeId::PNG
    )
}

fn validate_name(name: &str) -> io::Result<()> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid file/folder name",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct Point {
    x: f64,
    y: f64,
}

fn edge_anchor(
    from: (i32, i32),
    from_width: usize,
    toward: (i32, i32),
    toward_width: usize,
) -> Point {
    let fx = from.0 as f64 + from_width as f64 * 0.5;
    let fy = from.1 as f64 + 0.5;
    let tx = toward.0 as f64 + toward_width as f64 * 0.5;
    let ty = toward.1 as f64 + 0.5;
    let dx = tx - fx;
    let dy = ty - fy;

    if dx.abs() > dy.abs() * 1.7 {
        Point {
            x: if dx > 0.0 {
                from.0 as f64 + from_width as f64 + 0.15
            } else {
                from.0 as f64 - 0.15
            },
            y: fy,
        }
    } else {
        Point {
            x: fx,
            y: if dy > 0.0 {
                from.1 as f64 + 1.05
            } else {
                from.1 as f64 - 0.05
            },
        }
    }
}

/// Sparse world-space Braille mask cache. Each terminal cell owns a 2x4 dot
/// grid. It is rebuilt only when the filesystem or layout changes; camera motion
/// simply translates and clips the cached cells.
struct WorldBrailleCanvas {
    masks: HashMap<(i32, i32), u8>,
}

impl WorldBrailleCanvas {
    fn new() -> Self {
        Self {
            masks: HashMap::new(),
        }
    }

    fn into_cells(self) -> Vec<EdgeCell> {
        let mut cells: Vec<EdgeCell> = self
            .masks
            .into_iter()
            .filter_map(|((x, y), mask)| (mask != 0).then_some(EdgeCell { x, y, mask }))
            .collect();
        cells.sort_unstable_by_key(|cell| (cell.y, cell.x));
        cells
    }

    fn to_dot(point: Point) -> Point {
        Point {
            x: point.x * 2.0,
            y: point.y * 4.0,
        }
    }

    fn set_dot(&mut self, x: f64, y: f64) {
        let x = x.round() as i32;
        let y = y.round() as i32;
        let cell_x = x.div_euclid(2);
        let cell_y = y.div_euclid(4);
        let local_x = x.rem_euclid(2) as usize;
        let local_y = y.rem_euclid(4) as usize;
        let bit_table = [[0_u8, 3_u8], [1, 4], [2, 5], [6, 7]];
        let bit = bit_table[local_y][local_x];
        *self.masks.entry((cell_x, cell_y)).or_insert(0) |= 1 << bit;
    }

    fn draw_segment(&mut self, a: Point, b: Point) {
        let mut x0 = a.x.round() as i32;
        let mut y0 = a.y.round() as i32;
        let x1 = b.x.round() as i32;
        let y1 = b.y.round() as i32;
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        loop {
            self.set_dot(x0 as f64, y0 as f64);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = err * 2;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    fn sample_cubic(&mut self, a: Point, c1: Point, c2: Point, b: Point) {
        let distance = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let steps = ((distance / 1.9).ceil() as usize).clamp(10, 512);
        let mut previous = a;
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let mt = 1.0 - t;
            let point = Point {
                x: mt * mt * mt * a.x
                    + 3.0 * mt * mt * t * c1.x
                    + 3.0 * mt * t * t * c2.x
                    + t * t * t * b.x,
                y: mt * mt * mt * a.y
                    + 3.0 * mt * mt * t * c1.y
                    + 3.0 * mt * t * t * c2.y
                    + t * t * t * b.y,
            };
            self.draw_segment(previous, point);
            previous = point;
        }
    }

    fn sample_quadratic(&mut self, a: Point, c: Point, b: Point) {
        let distance = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let steps = ((distance / 2.1).ceil() as usize).clamp(8, 512);
        let mut previous = a;
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let mt = 1.0 - t;
            let point = Point {
                x: mt * mt * a.x + 2.0 * mt * t * c.x + t * t * b.x,
                y: mt * mt * a.y + 2.0 * mt * t * c.y + t * t * b.y,
            };
            self.draw_segment(previous, point);
            previous = point;
        }
    }

    fn draw_straight(&mut self, a: Point, b: Point) {
        self.draw_segment(Self::to_dot(a), Self::to_dot(b));
    }

    fn draw_elbow(&mut self, a: Point, b: Point) {
        let a = Self::to_dot(a);
        let b = Self::to_dot(b);
        let dx = b.x - a.x;
        let dy = b.y - a.y;

        if dx.abs() >= dy.abs() {
            let middle_x = ((a.x + b.x) * 0.5).round();
            let p1 = Point {
                x: middle_x,
                y: a.y,
            };
            let p2 = Point {
                x: middle_x,
                y: b.y,
            };
            self.draw_segment(a, p1);
            self.draw_segment(p1, p2);
            self.draw_segment(p2, b);
        } else {
            let middle_y = ((a.y + b.y) * 0.5).round();
            let p1 = Point {
                x: a.x,
                y: middle_y,
            };
            let p2 = Point {
                x: b.x,
                y: middle_y,
            };
            self.draw_segment(a, p1);
            self.draw_segment(p1, p2);
            self.draw_segment(p2, b);
        }
    }

    fn draw_soft_arc(&mut self, a: Point, b: Point) {
        let a = Self::to_dot(a);
        let b = Self::to_dot(b);
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let distance = (dx * dx + dy * dy).sqrt().max(1.0);
        let bend = (distance * 0.15).clamp(4.0, 18.0);
        let nx = -dy / distance;
        let ny = dx / distance;
        let sign = if a.x + b.x < 0.0 { -1.0 } else { 1.0 };
        let control = Point {
            x: (a.x + b.x) * 0.5 + nx * bend * sign,
            y: (a.y + b.y) * 0.5 + ny * bend * sign,
        };
        self.sample_quadratic(a, control, b);
    }

    fn draw_s_curve(&mut self, a: Point, b: Point) {
        let a = Self::to_dot(a);
        let b = Self::to_dot(b);
        let dx = b.x - a.x;
        let dy = b.y - a.y;

        if dx.abs() >= dy.abs() {
            self.sample_cubic(
                a,
                Point {
                    x: a.x + dx * 0.42,
                    y: a.y,
                },
                Point {
                    x: b.x - dx * 0.42,
                    y: b.y,
                },
                b,
            );
        } else {
            self.sample_cubic(
                a,
                Point {
                    x: a.x,
                    y: a.y + dy * 0.42,
                },
                Point {
                    x: b.x,
                    y: b.y - dy * 0.42,
                },
                b,
            );
        }
    }

    fn draw_radial_spline(&mut self, a: Point, b: Point) {
        let a = Self::to_dot(a);
        let b = Self::to_dot(b);
        let al = (a.x.powi(2) + a.y.powi(2)).sqrt().max(1.0);
        let bl = (b.x.powi(2) + b.y.powi(2)).sqrt().max(1.0);
        let au = Point {
            x: a.x / al,
            y: a.y / al,
        };
        let bu = Point {
            x: b.x / bl,
            y: b.y / bl,
        };
        let distance = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let reach = (distance * 0.36).max(5.0);

        self.sample_cubic(
            a,
            Point {
                x: a.x + au.x * reach,
                y: a.y + au.y * reach,
            },
            Point {
                x: b.x - bu.x * reach,
                y: b.y - bu.y * reach,
            },
            b,
        );
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn relative_age(when: SystemTime) -> String {
    let elapsed = elapsed_since(when);
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn archive_suffix_preserves_the_entire_archive_stem() {
        for (name, expected) in [
            ("assets.tar.mz4", "assets"),
            ("assets.tar.lz4", "assets"),
            ("My.Assets.TAR.MZ4", "My.Assets"),
        ] {
            let suffix = super::archive_suffix(name).unwrap();
            assert_eq!(&name[..name.len() - suffix.len()], expected);
        }
        assert!(super::archive_suffix("notes.txt").is_none());
    }

    #[test]
    fn archive_operation_polls_once_and_rejects_duplicate_start() {
        use super::*;
        use std::{cell::Cell, rc::Rc, task::Poll};
        let polls = Rc::new(Cell::new(0));
        let counter = polls.clone();
        let mut graph = GraphView {
            content_enabled: true,
            root: PathBuf::new(),
            nodes: Vec::new(),
            positions: Vec::new(),
            edge_cells: Vec::new(),
            camera_x: 0,
            camera_y: 0,
            column_gap: 18,
            layout_mode: LayoutMode::Tree,
            line_style: LineStyle::Default,
            suppressed_parent_edges: Vec::new(),
            depth_limit: DEFAULT_DEPTH_LIMIT,
            scene_revision: 0,
            pending_archives: vec![PendingArchive {
                future: Box::pin(std::future::poll_fn(move |_| {
                    counter.set(counter.get() + 1);
                    if counter.get() == 1 {
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(trueos::archive::ERR_TOO_LARGE))
                    }
                })),
                destination: "test.tar.lz4".into(),
                label: "LZ4",
                last_marker: std::time::Instant::now(),
                source: None,
                progress: std::rc::Rc::new(std::cell::Cell::new(0)),
                started: std::time::Instant::now(),
                next_poll: std::time::Instant::now(),
                displayed_percent: None,
            }],
        };
        assert!(graph.poll_archive(std::time::Instant::now() + crate::FALL_TICK).is_none());
        assert_eq!(polls.get(), 1);
        assert!(graph.archive_node(0).unwrap().contains("already running"));
        assert!(
            graph
                .archive_nodes(&[])
                .unwrap()
                .contains("already running")
        );
        assert_eq!(polls.get(), 1);
        let message = graph.poll_archive(std::time::Instant::now() + crate::FALL_TICK * 2).unwrap();
        assert!(message.starts_with("ARCHIVE FAILED"));
        assert!(message.contains("resource limit (-7)"));
        assert_eq!(polls.get(), 2);
        assert!(graph.pending_archives.is_empty());
        assert!(graph.poll_archive(std::time::Instant::now() + crate::FALL_TICK).is_none());

        // Two jobs remain independent, and only the polled cadence changes labels.
        let started = std::time::Instant::now();
        let done = Rc::new(Cell::new(false));
        for (id, percent) in [(0, 42), (1, 67)] {
            let path = PathBuf::from(format!("assets-{id}.tar.lz4"));
            graph.nodes.push(FsNode {
                id, parent: None, path: path.clone(), name: format!("assets-{id}.tar.lz4"),
                is_dir: false, content_type: trueos::content_identity::ContentTypeId::LZ4,
                is_placeholder: false, is_removed: false, is_moved: false, hidden: false, depth: 0,
            });
            let finished = done.clone();
            graph.pending_archives.push(PendingArchive {
                future: Box::pin(std::future::poll_fn(move |_| {
                    if id == 0 && finished.get() { Poll::Ready(Err(-2)) } else { Poll::Pending }
                })),
                destination: format!("assets-{id}"), label: "UNPACK", last_marker: started,
                source: Some(path), progress: Rc::new(Cell::new(percent)), started,
                next_poll: started, displayed_percent: Some(0),
            });
        }
        graph.poll_archive(started);
        assert_eq!(graph.visual_label(0), "🖹 42%");
        assert_eq!(graph.visual_label(1), "🖹 67%");
        assert!(graph.archive_node(0).unwrap().contains("already unpacking"));
        assert_eq!(graph.nodes[0].name, "assets-0.tar.lz4");
        let revision = graph.scene_revision();
        graph.pending_archives[0].progress.set(43);
        graph.poll_archive(started + crate::FALL_TICK / 2);
        assert_eq!(graph.visual_label(0), "🖹 42%");
        assert_eq!(graph.scene_revision(), revision);
        graph.poll_archive(started + crate::FALL_TICK);
        assert_eq!(graph.visual_label(0), "🖹 43%");
        graph.poll_archive(started + std::time::Duration::from_millis(3000));
        assert_eq!(graph.visual_label(0), "🖹 assets-0.tar.lz4");
        graph.poll_archive(started + std::time::Duration::from_millis(4000));
        assert_eq!(graph.visual_label(0), "🖹 43%");
        done.set(true);
        assert!(graph.poll_archive(started + std::time::Duration::from_millis(4090)).is_some());
        assert_eq!(graph.visual_label(0), "🖹 assets-0.tar.lz4");
        assert_eq!(graph.visual_label(1), "🖹 67%");
        assert_eq!(graph.pending_archives.len(), 1);
    }

    #[test]
    fn archive_label_alternates_three_seconds_of_progress_and_one_of_filename() {
        use std::time::Duration;
        for (ms, expected) in [(0, Some(42)), (2999, Some(42)), (3000, None),
            (3999, None), (4000, Some(42)), (7000, None), (8000, Some(42))] {
            assert_eq!(super::unpack_label_percent(Duration::from_millis(ms), 42), expected);
        }
    }

    #[test]
    fn edit_dispatch_requires_one_live_txt_file_and_preserves_its_disc() {
        use super::*;
        use crate::actions::{dispatch_menu_selected, Dispatch, MenuCommand};
        let mut graph = GraphView::new(Path::new("trueosfs:disc7"), false);
        graph.nodes.push(FsNode {
            id: 0, parent: None, path: PathBuf::from("trueosfs:disc7/My notes/note.TXT"),
            name: "note.TXT".into(), is_dir: false,
            content_type: trueos::content_identity::ContentTypeId::BLOB,
            is_placeholder: false, is_removed: false, is_moved: false, hidden: false, depth: 0,
        });
        for files in [vec![], vec![0]] {
            let result = dispatch_menu_selected(MenuCommand::Edit, &mut graph, Some(0), &files).unwrap();
            assert!(matches!(result, Dispatch::Edit(ref path) if path == Path::new("trueosfs:disc7/My notes/note.TXT")));
        }
        for (selected, files) in [(None, vec![]), (Some(0), vec![0, 1])] {
            assert!(matches!(dispatch_menu_selected(MenuCommand::Edit, &mut graph, selected, &files).unwrap(), Dispatch::Status(_)));
        }
        graph.nodes[0].path = PathBuf::from("trueosfs:disc7/note.md");
        assert!(!is_editable_text_file(&graph.nodes[0]));
        graph.nodes[0].path = PathBuf::from("trueosfs:disc7/note.txt");
        graph.nodes[0].is_dir = true;
        assert!(!is_editable_text_file(&graph.nodes[0]));
        graph.nodes[0].is_dir = false;
        graph.nodes[0].is_removed = true;
        assert!(!is_editable_text_file(&graph.nodes[0]));
        graph.nodes[0].is_removed = false;
        graph.nodes[0].hidden = true;
        assert!(!is_editable_text_file(&graph.nodes[0]));
    }

    #[test]
    fn video_dispatch_uses_inferred_identity_and_preserves_disc_selector() {
        use super::*;
        use trueos::content_identity::ContentTypeId;
        let mut graph = GraphView {
            content_enabled: true,
            root: PathBuf::new(),
            nodes: vec![FsNode {
                id: 0,
                parent: None,
                path: PathBuf::from("trueosfs:disc7/movie.bin"),
                name: "movie.bin".into(),
                is_dir: false,
                content_type: ContentTypeId::MATROSKA,
                is_placeholder: false,
                is_removed: false,
                is_moved: false,
                hidden: false,
                depth: 0,
            }],
            positions: Vec::new(),
            edge_cells: Vec::new(),
            camera_x: 0,
            camera_y: 0,
            column_gap: 18,
            layout_mode: LayoutMode::Tree,
            line_style: LineStyle::Default,
            suppressed_parent_edges: Vec::new(),
            depth_limit: DEFAULT_DEPTH_LIMIT,
            scene_revision: 0,
            pending_archives: Vec::new(),
        };
        for content_type in [ContentTypeId::MATROSKA, ContentTypeId::MP4] {
            graph.nodes[0].content_type = content_type;
            assert_eq!(
                graph.video_path_for_node(0).unwrap(),
                "trueosfs:disc7/movie.bin"
            );
        }
        graph.nodes[0].path = PathBuf::from("trueosfs:disc7/movie.mkv");
        graph.nodes[0].content_type = ContentTypeId::BLOB;
        assert!(graph.video_path_for_node(0).is_err());
        graph.nodes[0].content_type = ContentTypeId::MATROSKA;
        graph.nodes[0].is_removed = true;
        assert!(graph.video_path_for_node(0).is_err());
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}

#[cfg(test)]
mod native_chrome_tests {
    use super::*;
    #[test]
    fn native_chrome_validated_mount_root_changes_keep_content_disabled() {
        let mut graph = GraphView::empty(Path::new("/initial-root"));
        // mount_path performs VFS validation, then sets this root and reloads.
        graph.root = PathBuf::from("/validated-mount-root");
        graph.reload().unwrap();
        assert_eq!(graph.root_path(), Path::new("/validated-mount-root"));
        assert_eq!(graph.nodes.len(), 1);
        assert!(graph.edge_cells.is_empty());
        assert!(graph.minimap_samples().is_empty());
    }
}
