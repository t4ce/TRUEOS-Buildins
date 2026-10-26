use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, Default)]
pub struct WorldPos {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutMode {
    Tree,
    Radial,
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutNode {
    pub id: usize,
    pub parent: Option<usize>,
    pub depth: usize,
    pub visual_width: usize,
    pub is_dir: bool,
}

const DENSE_CHILD_CAP: usize = 32;
const DENSE_LANE_GAP: i32 = 4;
const FOLDER_ROW_DISTANCE: i32 = 3; // two empty terminal rows between folder roots

/// Compact left-to-right forest layout.
///
/// The mount root (node 0) is structural and hidden. Its children are laid out
/// as independent islands. Files may sit on adjacent rows. Folder sibling roots
/// keep two empty rows between them because folder drop hitboxes extend one row
/// above/below the visible label. Large sibling sets wrap after 32 children into
/// additional X lanes instead of becoming an unbounded vertical wall.
pub fn tree_layout(nodes: &[LayoutNode], column_gap: i32) -> Vec<WorldPos> {
    if nodes.is_empty() {
        return Vec::new();
    }

    let children = child_table(nodes);
    let depth_x = depth_columns(nodes, column_gap.max(2));
    let mut engine = TreeEngine {
        nodes,
        children,
        depth_x,
        positions: vec![WorldPos::default(); nodes.len()],
    };

    engine.place_subtree(0, 0, 0);

    // Normalize around the hidden mount root so camera (0,0) is a stable center
    // while the first visible generation still begins at world x=0.
    let root = engine.positions[0];
    for pos in &mut engine.positions {
        pos.x -= root.x;
        pos.y -= root.y;
    }

    engine.positions
}

/// Weighted radial forest inspired by the attached Braille prototype.
///
/// Direct children of the hidden mount root own angular sectors proportional to
/// subtree leaf weight. Descendants remain inside a narrowing local sector and
/// move outward by depth. X is stretched because terminal cells are taller than
/// they are wide, which makes the result appear much closer to circular.
pub fn radial_layout(nodes: &[LayoutNode], spacing: i32) -> Vec<WorldPos> {
    if nodes.is_empty() {
        return Vec::new();
    }

    let children = child_table(nodes);
    let mut positions = vec![WorldPos::default(); nodes.len()];
    let radial_step = (spacing.max(12) as f64 * 0.55).max(7.0);
    let x_aspect = 2.15_f64;
    let y_aspect = 0.88_f64;

    let top = &children[0];
    if top.is_empty() {
        return positions;
    }

    // Grow the first visible ring when the mount itself has many children.
    // Radial world-space is unbounded, so a wide ring is preferable to labels
    // colliding around a tiny fixed circumference.
    let top_units = top
        .iter()
        .map(|&id| nodes[id].visual_width.clamp(3, 24) + 2)
        .sum::<usize>() as f64;
    let base_radius = radial_step.max(top_units / (PI * 2.0 * 1.35));

    let weights: Vec<usize> = top
        .iter()
        .map(|&id| subtree_weight(id, &children))
        .collect();
    let total = weights.iter().sum::<usize>().max(1) as f64;

    // -PI makes a single top-level island start on the right rather than below
    // the camera center, while multiple islands still fill the full circle.
    let mut cursor = -PI;
    for (&child, &weight) in top.iter().zip(weights.iter()) {
        let span = (weight as f64 / total) * PI * 2.0;
        place_radial(
            child,
            cursor,
            cursor + span,
            &children,
            nodes,
            &mut positions,
            base_radius,
            radial_step,
            x_aspect,
            y_aspect,
        );
        cursor += span;
    }

    positions
}

fn child_table(nodes: &[LayoutNode]) -> Vec<Vec<usize>> {
    let mut children = vec![Vec::new(); nodes.len()];
    for node in nodes {
        if let Some(parent) = node.parent {
            if parent < children.len() {
                children[parent].push(node.id);
            }
        }
    }
    children
}

fn depth_columns(nodes: &[LayoutNode], column_gap: i32) -> Vec<i32> {
    let max_depth = nodes.iter().map(|n| n.depth).max().unwrap_or(0);
    let mut widths = vec![1_i32; max_depth + 1];
    for node in nodes {
        widths[node.depth] = widths[node.depth].max(node.visual_width as i32);
    }

    let mut x = vec![0_i32; max_depth + 1];
    if max_depth >= 1 {
        // The hidden root does not consume a visible column.
        x[1] = 0;
    }
    for depth in 2..=max_depth {
        x[depth] = x[depth - 1] + widths[depth - 1] + column_gap;
    }
    x
}

#[derive(Clone, Copy, Debug)]
struct Placement {
    root_y: i32,
    bottom_y: i32,
    max_x: i32,
}

struct TreeEngine<'a> {
    nodes: &'a [LayoutNode],
    children: Vec<Vec<usize>>,
    depth_x: Vec<i32>,
    positions: Vec<WorldPos>,
}

impl TreeEngine<'_> {
    fn place_subtree(&mut self, id: usize, top_y: i32, x_bias: i32) -> Placement {
        let node = self.nodes[id];
        let x = self.depth_x.get(node.depth).copied().unwrap_or(0) + x_bias;
        let child_ids = self.children[id].clone();

        if child_ids.is_empty() {
            self.positions[id] = WorldPos { x, y: top_y };
            return Placement {
                root_y: top_y,
                bottom_y: top_y,
                max_x: x + node.visual_width as i32,
            };
        }

        let group = if child_ids.len() <= DENSE_CHILD_CAP {
            self.place_linear_children(&child_ids, top_y, x_bias)
        } else {
            self.place_dense_children(&child_ids, top_y, x_bias)
        };

        self.positions[id] = WorldPos { x, y: group.root_y };

        Placement {
            root_y: group.root_y,
            bottom_y: group.bottom_y.max(group.root_y),
            max_x: group.max_x.max(x + node.visual_width as i32),
        }
    }

    fn place_linear_children(&mut self, children: &[usize], top_y: i32, x_bias: i32) -> Placement {
        let mut cursor = top_y;
        let mut previous_folder_y: Option<i32> = None;
        let mut first_root = None;
        let mut last_root = top_y;
        let mut bottom = top_y;
        let mut max_x = i32::MIN;

        for &child in children {
            let mut placed = self.place_subtree(child, cursor, x_bias);

            if self.nodes[child].is_dir {
                if let Some(previous) = previous_folder_y {
                    let required = previous + FOLDER_ROW_DISTANCE;
                    if placed.root_y < required {
                        let delta = required - placed.root_y;
                        self.shift_subtree(child, delta);
                        placed.root_y += delta;
                        placed.bottom_y += delta;
                    }
                }
                previous_folder_y = Some(placed.root_y);
            }

            first_root.get_or_insert(placed.root_y);
            last_root = placed.root_y;
            bottom = bottom.max(placed.bottom_y);
            max_x = max_x.max(placed.max_x);

            // Files may remain directly adjacent. A folder owns a visual
            // subtree/island, so leave one empty row after its full extent before
            // placing the next sibling block. This keeps neighbouring hierarchy
            // groups from visually welding together without bloating file lists.
            cursor = placed.bottom_y + if self.nodes[child].is_dir { 2 } else { 1 };
        }

        let first = first_root.unwrap_or(top_y);
        Placement {
            root_y: (first + last_root) / 2,
            bottom_y: bottom,
            max_x: if max_x == i32::MIN { 0 } else { max_x },
        }
    }

    fn place_dense_children(&mut self, children: &[usize], top_y: i32, x_bias: i32) -> Placement {
        let mut lane_bias = 0_i32;
        let mut root_min = i32::MAX;
        let mut root_max = i32::MIN;
        let mut bottom = top_y;
        let mut max_x = i32::MIN;

        for lane in children.chunks(DENSE_CHILD_CAP) {
            let placed = self.place_linear_children(lane, top_y, x_bias + lane_bias);
            root_min = root_min.min(placed.root_y);
            root_max = root_max.max(placed.root_y);
            bottom = bottom.max(placed.bottom_y);
            max_x = max_x.max(placed.max_x);

            let first = lane[0];
            let direct_x = self.depth_x[self.nodes[first].depth] + x_bias + lane_bias;
            let lane_extent = (placed.max_x - direct_x).max(1);
            lane_bias += lane_extent + DENSE_LANE_GAP;
        }

        Placement {
            root_y: if root_min == i32::MAX {
                top_y
            } else {
                (root_min + root_max) / 2
            },
            bottom_y: bottom,
            max_x: if max_x == i32::MIN { 0 } else { max_x },
        }
    }

    fn shift_subtree(&mut self, id: usize, dy: i32) {
        self.positions[id].y += dy;
        let children = self.children[id].clone();
        for child in children {
            self.shift_subtree(child, dy);
        }
    }
}

fn subtree_weight(id: usize, children: &[Vec<usize>]) -> usize {
    if children[id].is_empty() {
        return 1;
    }
    children[id]
        .iter()
        .map(|&child| subtree_weight(child, children))
        .sum::<usize>()
        .max(1)
}

fn place_radial(
    id: usize,
    start: f64,
    end: f64,
    children: &[Vec<usize>],
    nodes: &[LayoutNode],
    positions: &mut [WorldPos],
    base_radius: f64,
    radial_step: f64,
    x_aspect: f64,
    y_aspect: f64,
) {
    let node = nodes[id];
    let angle = (start + end) * 0.5;
    let radius = base_radius + radial_step * node.depth.saturating_sub(1) as f64;
    let cx = angle.cos() * radius * x_aspect;
    let cy = angle.sin() * radius * y_aspect;

    positions[id] = WorldPos {
        x: (cx - node.visual_width as f64 * 0.5).round() as i32,
        y: cy.round() as i32,
    };

    let kids = &children[id];
    if kids.is_empty() {
        return;
    }

    // A single huge sector is narrowed around its parent's heading so children
    // continue outward rather than curling all the way back toward the center.
    let midpoint = angle;
    let local_span = (end - start).abs().min(PI * 0.92).max(0.18);
    let local_start = midpoint - local_span * 0.5;
    let local_end = midpoint + local_span * 0.5;

    let weights: Vec<usize> = kids
        .iter()
        .map(|&child| subtree_weight(child, children))
        .collect();
    let total = weights.iter().sum::<usize>().max(1) as f64;
    let mut cursor = local_start;

    for (&child, &weight) in kids.iter().zip(weights.iter()) {
        let span = (weight as f64 / total) * (local_end - local_start);
        place_radial(
            child,
            cursor,
            cursor + span,
            children,
            nodes,
            positions,
            base_radius,
            radial_step,
            x_aspect,
            y_aspect,
        );
        cursor += span;
    }
}

/// Marks direct child edges that can be omitted in tree mode after a dense sibling
/// set wraps into additional X lanes. The first 32-child lane keeps the visual
/// parent connection; later lanes remain internally connected, but their redundant
/// long back-link to the same parent is suppressed.
pub fn tree_suppressed_parent_edges(nodes: &[LayoutNode]) -> Vec<bool> {
    let children = child_table(nodes);
    let mut suppressed = vec![false; nodes.len()];
    for siblings in children {
        if siblings.len() <= DENSE_CHILD_CAP {
            continue;
        }
        for &child in siblings.iter().skip(DENSE_CHILD_CAP) {
            if child < suppressed.len() {
                suppressed[child] = true;
            }
        }
    }
    suppressed
}
