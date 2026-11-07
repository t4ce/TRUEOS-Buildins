//! Typed Lightning CSS -> Taffy bridge, with token substitution before property resolution.
#[cfg(target_os = "trueos")]
use num_traits::Float;
use alloc::{string::{String, ToString}, vec::Vec, borrow::ToOwned, format, vec};
use super::css::Css;
use crate::selectors::{MatchDom, matches_components};
#[path = "custom_properties.rs"]
mod custom_properties;
use custom_properties::Variables;
use lightningcss::properties::{
    Property,
    display::{Display as CssDisplay, DisplayInside, DisplayKeyword},
    font::{
        AbsoluteFontSize, FontSize as CssFontSize, LineHeight as CssLineHeight, RelativeFontSize,
    },
    size::Size as CssSize,
};
use lightningcss::values::length::{
    LengthPercentage as CssLengthPercentage, LengthPercentageOrAuto, LengthValue,
};
use rp5::{Document, NodeId as HtmlId, NodeKind};
use hashbrown::HashMap;
use core::fmt::Write;
use taffy::prelude::*;

#[derive(Debug)]
pub(super) enum LayoutError {
    Tree(taffy::TaffyError),
    Diagnostics(serde_json::Error),
}

impl core::fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Tree(error) => core::fmt::Display::fmt(error, formatter),
            Self::Diagnostics(error) => core::fmt::Display::fmt(error, formatter),
        }
    }
}

impl From<taffy::TaffyError> for LayoutError {
    fn from(error: taffy::TaffyError) -> Self { Self::Tree(error) }
}

impl From<serde_json::Error> for LayoutError {
    fn from(error: serde_json::Error) -> Self { Self::Diagnostics(error) }
}

pub(super) struct Export {
    pub json: String,
    pub svg: String,
    pub line_rects: Vec<[f32; 4]>,
    pub background_quads: Vec<BackgroundQuad>,
    pub canvas_color: [u8; 4],
    pub border_quads: Vec<BorderQuad>,
    pub button_boxes: Vec<ButtonBox>,
    pub line_extent: (u32, u32),
    pub text_runs: Vec<TextRun>,
    /// Frozen content boxes. Image completion changes paint only.
    pub image_boxes: Vec<ImageBox>,
    pub link_boxes: Vec<LinkBox>,
}

/// One uniform native line loop per diagnostic element bounding box.
#[derive(Clone, Copy, Debug)]
pub(super) struct BorderQuad {
    pub rect: [f32; 4],
    pub color: [u8; 4],
    /// Top-left, top-right, bottom-right, bottom-left; x/y in pixels.
    pub radii: [[f32; 2]; 4],
}

#[derive(Clone, Copy, Default)]
struct Radius {
    pixels: f32,
    fraction: f32,
}

/// Stable document-space button bounds for native cursor feedback.
#[derive(Clone, Copy, Debug)]
pub(super) struct ButtonBox {
    pub rect: [f32; 4],
    pub border_color: [u8; 4],
}

/// Solid CSS background, in document coordinates and DOM paint order.
#[derive(Clone, Copy, Debug)]
pub(super) struct BackgroundQuad {
    pub rect: [f32; 4],
    pub color: [u8; 4],
}

/// A clickable fragment, in the same document coordinates as its paint.
#[derive(Clone, Copy, Debug)]
pub(super) struct LinkBox {
    pub node: HtmlId,
    pub rect: [f32; 4],
    /// Only painted text fragments receive hover ink; broad click boxes do not.
    pub hover_color: Option<[u8; 4]>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ImageBox {
    pub node: HtmlId,
    pub rect: [f32; 4],
}

pub(super) const MAX_IMAGE_EDGE: f32 = 1024.0;
const MAX_LAYOUT_LENGTH: f32 = 32_768.0;
const MAX_DOCUMENT_EDGE: f32 = 1_000_000.0;

// Page text grows before measurement and wrapping, independently of viewport
// pixels, physical distance units, window chrome, and the terminal font.
const INITIAL_FONT_SIZE: f32 = 11.0;
const PAGE_TEXT_SCALE: f32 = 2.0;
const DEFAULT_FONT_SIZE: f32 = INITIAL_FONT_SIZE * PAGE_TEXT_SCALE;

/// The same baseline and textLength geometry written into the SVG snapshot.
pub(super) struct TextRun {
    pub text: String,
    pub x: f32,
    pub baseline_y: f32,
    pub size_px: f32,
    pub advance_px: f32,
    pub color: [u8; 4],
}

// Host sandbox and layout tests read the diagnostic JSON; the device build renders with compute_for_render.
#[allow(dead_code)]
pub(super) fn compute(
    document: &Document,
    css: &Css,
    viewport: (u32, u32),
) -> Result<Export, LayoutError> {
    export(document, css, viewport, true)
}

/// Render path: skips the diagnostic JSON, which only the host sandbox reads.
pub(super) fn compute_for_render(
    document: &Document,
    css: &Css,
    viewport: (u32, u32),
) -> Result<Export, LayoutError> {
    export(document, css, viewport, false)
}

fn export(
    document: &Document,
    css: &Css,
    viewport: (u32, u32),
    diagnostics: bool,
) -> Result<Export, LayoutError> {
    let started = crate::execution::Instant::now();
    let mut phase_started = started;
    let mut phase = |name: &str| {
        let now = crate::execution::Instant::now();
        trueos::logl::log(
            trueos::logl::level::INFO,
            format_args!(
                "hull: layout phase={} elapsed_ms={} total_ms={}",
                name,
                now.duration_since(phase_started).as_millis(),
                now.duration_since(started).as_millis(),
            ),
        );
        phase_started = now;
    };
    let match_dom = MatchDom::new(document);
    let cascade = CascadeIndex::new(
        css,
        ResolveContext {
            viewport,
            font_size: INITIAL_FONT_SIZE,
            root_font_size: INITIAL_FONT_SIZE,
        },
    );
    let mut cascade_scratch = CascadeScratch::default();
    let mut tree: TaffyTree<TextMeasureContext> = TaffyTree::new();
    let mut ids: Vec<Vec<NodeId>> = vec![vec![]; document.nodes.len()];
    let mut owners = HashMap::new();
    let mut inline = vec![false; document.nodes.len()];
    let mut contents = vec![false; document.nodes.len()];
    let mut hidden = vec![false; document.nodes.len()];
    let mut link_owners = vec![None; document.nodes.len()];
    let mut styles = vec![Style::default(); document.nodes.len()];
    let mut text_metrics = vec![TextMetrics::default(); document.nodes.len()];
    let mut variables = vec![Variables::default(); document.nodes.len()];
    let mut root_font_size = DEFAULT_FONT_SIZE;
    phase("setup");
    let mut pending = vec![document.root];
    while let Some(id) = pending.pop() {
        let node = &document.nodes[id];
        link_owners[id] = if document.name(id) == "a" && super::links::href(document, id).is_some()
        {
            Some(id)
        } else {
            node.parent.and_then(|parent| link_owners[parent])
        };
        let inherited = node
            .parent
            .map(|parent| text_metrics[parent])
            .unwrap_or_default();
        let mut inherited_variables = node
            .parent
            .map(|parent| variables[parent].clone())
            .unwrap_or_default();
        let mut metrics = inherited;
        // Background colors belong to the element; they are never inherited.
        metrics.background = [0; 4];
        metrics.border_color = [0, 0, 0, 255];
        metrics.border_radii = [[Radius::default(); 2]; 4];
        inline[id] = matches!(&node.kind, NodeKind::Text(_))
            || document
                .element(id)
                .is_some_and(|(name, _, _)| default_inline(name));
        if document.name(id) == "pre" {
            metrics.whitespace = WhiteSpaceMetric::Pre;
        }
        let mut style = Style {
            display: Display::Block,
            box_sizing: BoxSizing::ContentBox,
            ..Style::default()
        };
        match &node.kind {
            NodeKind::Element { name, attrs, .. } => {
                // Native button baseline; authored CSS below still wins.
                if name == "button"
                    || (name == "input"
                        && attrs.iter().any(|a| {
                            a.name == "type"
                                && ["button", "submit", "reset"]
                                    .iter()
                                    .any(|kind| a.value.eq_ignore_ascii_case(kind))
                        }))
                {
                    let width = LengthPercentage::from_length(3.0);
                    style.border = Rect {
                        top: width,
                        right: width,
                        bottom: width,
                        left: width,
                    };
                    metrics.background = [255; 4];
                }
                // Minimal link default; authored CSS still wins below.
                if name == "a" && attrs.iter().any(|attr| attr.name == "href") {
                    metrics.color = [0, 0, 255, 255];
                }
                if name == "video" {
                    metrics.background = [144, 238, 144, 255]; // lightgreen
                }
                let context = ResolveContext {
                    viewport,
                    font_size: inherited.font_size,
                    root_font_size,
                };
                apply_css(
                    document,
                    id,
                    css,
                    &cascade,
                    &match_dom,
                    &mut cascade_scratch,
                    &mut style,
                    &mut metrics,
                    &mut inherited_variables,
                    context,
                    &mut inline[id],
                    &mut contents[id],
                );
                if name == "img" {
                    image_style(&mut style, attrs, viewport);
                    // A replaced element always owns its image content box.
                    contents[id] = false;
                }
                if name == "html" {
                    root_font_size = metrics.font_size;
                }
                if matches!(
                    name.as_str(),
                    "head" | "script" | "style" | "meta" | "link" | "title" | "base" | "template"
                ) || attrs.iter().any(|attr| attr.name == "hidden")
                {
                    style.display = Display::None;
                }
            }
            NodeKind::Document | NodeKind::Fragment => {
                style.size.width = Dimension::from_length(viewport.0 as f32);
            }
            _ => {}
        }
        hidden[id] =
            style.display == Display::None || node.parent.is_some_and(|parent| hidden[parent]);
        styles[id] = style;
        text_metrics[id] = metrics;
        variables[id] = inherited_variables;
        pending.extend(node.children.iter().rev().copied());
    }
    phase("cascade");

    // Anonymous flex line boxes provide a first inline-flow implementation.
    // Word fragments retain their HTML owner while anonymous boxes stay out of the DOM.
    let mut pending = vec![(document.root, false)];
    while let Some((id, visited)) = pending.pop() {
        if !visited {
            pending.push((id, true));
            pending.extend(
                document.nodes[id]
                    .children
                    .iter()
                    .rev()
                    .map(|&child| (child, false)),
            );
            continue;
        }
        let node = &document.nodes[id];
        match &node.kind {
            NodeKind::Text(text) if !hidden[id] => {
                for text in text_chunks(text, text_metrics[id].whitespace) {
                    let pre_wrap = matches!(text_metrics[id].whitespace, WhiteSpaceMetric::PreWrap);
                    let style = Style {
                        display: Display::Block,
                        flex_shrink: if pre_wrap { 1.0 } else { 0.0 },
                        // Preserved text is one leaf, unlike normal word
                        // fragments. Permit it to shrink to the line width so
                        // its measurement and output can actually wrap.
                        min_size: Size {
                            width: if pre_wrap {
                                LengthPercentageAuto::from_length(0.0)
                            } else {
                                LengthPercentageAuto::AUTO
                            },
                            height: LengthPercentageAuto::AUTO,
                        },
                        ..Style::default()
                    };
                    let tid = tree.new_leaf_with_context(
                        style,
                        TextMeasureContext {
                            text,
                            metrics: text_metrics[id],
                        },
                    )?;
                    owners.insert(tid, id);
                    ids[id].push(tid);
                }
            }
            NodeKind::Element { .. } | NodeKind::Document | NodeKind::Fragment => {
                let mut style = styles[id].clone();
                let mut children = Vec::new();
                let mut run = Vec::new();
                let is_line_container =
                    inline[id] && style.display == Display::Block && document.name(id) != "img";
                if is_line_container {
                    style.display = Display::Flex;
                    style.flex_wrap = FlexWrap::Wrap;
                    style.flex_shrink = 0.0;
                    style.align_items = AlignItems::START;
                    style.justify_content = text_metrics[id].align.justify();
                }
                for &child in &node.children {
                    if contents[id] || is_line_container {
                        children.extend(ids[child].iter().copied());
                    } else if style.display != Display::Block {
                        if matches!(document.nodes[child].kind, NodeKind::Text(_)) {
                            run.extend(ids[child].iter().copied());
                        } else {
                            flush_inline_run(
                                &mut tree,
                                &mut children,
                                &mut run,
                                text_metrics[id],
                                false,
                            )?;
                            children.extend(ids[child].iter().copied());
                        }
                    } else if inline[child] {
                        run.extend(ids[child].iter().copied());
                    } else {
                        flush_inline_run(
                            &mut tree,
                            &mut children,
                            &mut run,
                            text_metrics[id],
                            style.display == Display::Block,
                        )?;
                        children.extend(ids[child].iter().copied());
                    }
                }
                flush_inline_run(
                    &mut tree,
                    &mut children,
                    &mut run,
                    text_metrics[id],
                    style.display == Display::Block,
                )?;
                if contents[id] {
                    ids[id] = children;
                } else {
                    let tid = tree.new_with_children(style, &children)?;
                    owners.insert(tid, id);
                    ids[id].push(tid);
                }
            }
            _ => {}
        }
    }
    phase("tree");
    let root = ids[document.root][0];
    tree.compute_layout_with_measure(
        root,
        Size {
            width: AvailableSpace::Definite(viewport.0 as f32),
            height: AvailableSpace::Definite(viewport.1 as f32),
        },
        |inputs, _, context, style| {
            taffy::compute_leaf_layout(
                inputs,
                style,
                |_, _| 0.0,
                |known_dimensions, available_space| {
                    let fallback = TextMetrics::default();
                    let (text, metrics) = context
                        .as_deref()
                        .map(|context| (context.text.as_str(), context.metrics))
                        .unwrap_or(("", fallback));
                    let natural = if matches!(
                        metrics.whitespace,
                        WhiteSpaceMetric::Pre | WhiteSpaceMetric::PreWrap
                    ) {
                        text.split('\n')
                            .map(|line| estimate_text_width(line, metrics.font_size))
                            .fold(0.0, f32::max)
                    } else {
                        estimate_text_width(text, metrics.font_size)
                    };
                    let limit =
                        known_dimensions
                            .width
                            .unwrap_or_else(|| match available_space.width {
                                AvailableSpace::Definite(width) => width,
                                AvailableSpace::MinContent => text
                                    .split_whitespace()
                                    .map(|word| estimate_text_width(word, metrics.font_size))
                                    .fold(0.0f32, f32::max),
                                AvailableSpace::MaxContent => natural,
                            });
                    let width = known_dimensions
                        .width
                        .unwrap_or(natural.min(limit.max(1.0)));
                    let height = known_dimensions.height.unwrap_or_else(|| {
                        visual_line_count(text, metrics.whitespace, width, metrics.font_size) as f32
                            * metrics.line_height.used(metrics.font_size)
                    });
                    Size { width, height }
                },
            )
        },
    )?;
    phase("taffy");
    let mut fragments: Vec<Vec<BoxRect>> = vec![vec![]; document.nodes.len()];
    let mut text_svg = String::new();
    let mut text_runs = Vec::new();
    let mut image_boxes = Vec::new();
    let mut link_boxes = Vec::new();
    let mut pending = vec![(root, 0.0f32, 0.0f32)];
    while let Some((tid, parent_x, parent_y)) = pending.pop() {
        let box_layout = tree.layout(tid)?;
        let left = project_position(parent_x + box_layout.location.x);
        let top = project_position(parent_y + box_layout.location.y);
        if let Some(&id) = owners.get(&tid) {
            if !hidden[id] && !contents[id] {
                fragments[id].push(BoxRect {
                    left,
                    top,
                    width: project_extent(box_layout.size.width),
                    height: project_extent(box_layout.size.height),
                });
                if text_metrics[id].paints()
                    && (link_owners[id] == Some(id) || document.name(id) == "img")
                {
                    if let Some(node) = link_owners[id] {
                        link_boxes.push(LinkBox {
                            node,
                            rect: [left, top, box_layout.size.width, box_layout.size.height],
                            hover_color: None,
                        });
                    }
                }
                if document.name(id) == "img" && text_metrics[id].paints() {
                    let horizontal = box_layout.border.left + box_layout.padding.left;
                    let vertical = box_layout.border.top + box_layout.padding.top;
                    image_boxes.push(ImageBox {
                        node: id,
                        rect: [
                            project_position(left + horizontal),
                            project_position(top + vertical),
                            (box_layout.size.width
                                - horizontal
                                - box_layout.border.right
                                - box_layout.padding.right)
                                .clamp(0.0, MAX_IMAGE_EDGE),
                            (box_layout.size.height
                                - vertical
                                - box_layout.border.bottom
                                - box_layout.padding.bottom)
                                .clamp(0.0, MAX_IMAGE_EDGE),
                        ],
                    });
                }
                if let Some(context) = tree.get_node_context(tid) {
                    if !context.metrics.paints()
                        || !context.metrics.font_size.is_finite()
                        || context.metrics.font_size <= 0.0
                    {
                        continue;
                    }
                    let line_height = context.metrics.line_height.used(context.metrics.font_size);
                    // Pixel snapping must not change the line breaks chosen
                    // during measurement (e.g. 75.6px becoming a 75px box).
                    let text_width = tree.unrounded_layout(tid).size.width;
                    for (line_index, line) in visual_lines(context, text_width).iter().enumerate() {
                        // Whitespace retains its measured advance and line
                        // boxes, but has no ink to export or submit to UI4.
                        if line.chars().all(is_css_space) {
                            continue;
                        }
                        let y = top
                            + line_index as f32 * line_height
                            + (line_height - context.metrics.font_size) / 2.0
                            + context.metrics.font_size * 0.8;
                        let length = estimate_text_width(line, context.metrics.font_size);
                        if let Some(node) = link_owners[id] {
                            link_boxes.push(LinkBox {
                                node,
                                rect: [
                                    left,
                                    top + line_index as f32 * line_height,
                                    length,
                                    line_height,
                                ],
                                hover_color: Some(context.metrics.color),
                            });
                        }
                        let [r, g, b, a] = context.metrics.color;
                        let _ = write!(
                            text_svg,
                            "<text x=\"{left}\" y=\"{y}\" font-size=\"{}\" font-family=\"monospace\" fill=\"#{r:02x}{g:02x}{b:02x}\" fill-opacity=\"{}\" xml:space=\"preserve\" textLength=\"{length}\" lengthAdjust=\"spacingAndGlyphs\">{}</text>",
                            context.metrics.font_size,
                            f32::from(a) / 255.0,
                            xml_escape(line)
                        );
                        text_runs.push(TextRun {
                            text: line.clone(),
                            x: left,
                            baseline_y: y,
                            size_px: context.metrics.font_size,
                            advance_px: length,
                            color: context.metrics.color,
                        });
                    }
                }
            }
        }
        pending.extend(
            tree.children(tid)?
                .into_iter()
                .rev()
                .map(|child| (child, left, top)),
        );
    }
    phase("text-svg");
    let mut rows = Vec::new();
    let mut rectangles = Vec::new();
    // HTML's root background paints the canvas. With a transparent root,
    // propagate the body's background instead, without painting it twice.
    let canvas_node = ["html", "body"].into_iter().find_map(|name| {
        (0..document.nodes.len()).find(|&id| {
            document.name(id) == name
                && !hidden[id]
                && !contents[id]
                && text_metrics[id].paints()
                && text_metrics[id].background[3] != 0
        })
    });
    let [r, g, b, a] = canvas_node
        .map(|id| text_metrics[id].background)
        .unwrap_or([0; 4]);
    let [r, g, b] = [r, g, b].map(|value| {
        ((u32::from(value) * u32::from(a) + 255 * (255 - u32::from(a)) + 127) / 255) as u8
    });
    let canvas_color = [r, g, b, 255];
    let mut background_quads = Vec::new();
    let mut border_quads = Vec::new();
    let mut button_boxes = Vec::new();
    let mut pending = vec![document.root];
    while let Some(id) = pending.pop() {
        if matches!(
            document.nodes[id].kind,
            NodeKind::Element { .. } | NodeKind::Text(_)
        ) {
            let tag = if matches!(document.nodes[id].kind, NodeKind::Text(_)) {
                "#text"
            } else {
                document.name(id)
            };
            let rect = BoxRect::bounds(&fragments[id]);
            if diagnostics {
                rows.push(serde_json::json!({"node":id,"tag":tag,"rect":rect.as_ref().map(BoxRect::json),"hidden":hidden[id],"paint_visible":!hidden[id] && text_metrics[id].paints(),"display_contents":contents[id],"text_metrics":{"font_size":text_metrics[id].font_size,"advance":text_metrics[id].font_size*6.0/11.0,"line_height":text_metrics[id].line_height.used(text_metrics[id].font_size)},"fragments":fragments[id].iter().map(BoxRect::json).collect::<Vec<_>>() }));
            }
            if text_metrics[id].paints()
                && matches!(document.nodes[id].kind, NodeKind::Element { .. })
            {
                if let Some(rect) = rect {
                    rectangles.push((id, rect.left, rect.top, rect.width, rect.height));
                    if !hidden[id]
                        && !contents[id]
                        && document.element(id).is_some_and(|(tag, _, attrs)| {
                            !attrs.iter().any(|a| a.name == "disabled")
                                && (tag == "button"
                                    || (tag == "input"
                                        && attrs.iter().any(|a| {
                                            a.name == "type"
                                                && ["button", "submit", "reset"]
                                                    .iter()
                                                    .any(|kind| a.value.eq_ignore_ascii_case(kind))
                                        })))
                        })
                    {
                        button_boxes.push(ButtonBox {
                            rect: [rect.left, rect.top, rect.width, rect.height],
                            border_color: text_metrics[id].border_color,
                        });
                    }
                    let border = styles[id].border;
                    if [border.top, border.right, border.bottom, border.left]
                        .iter()
                        .any(|width| *width != taffy::style::LengthPercentage::from_length(0.0))
                    {
                        border_quads.push(BorderQuad {
                            rect: [rect.left, rect.top, rect.width, rect.height],
                            color: text_metrics[id].border_color,
                            radii: text_metrics[id].border_radii.map(|[x, y]| {
                                [
                                    x.pixels + x.fraction * rect.width,
                                    y.pixels + y.fraction * rect.height,
                                ]
                            }),
                        });
                    }
                }
                if Some(id) != canvas_node && !contents[id] && text_metrics[id].background[3] != 0 {
                    for rect in &fragments[id] {
                        background_quads.push(BackgroundQuad {
                            rect: [rect.left, rect.top, rect.width, rect.height],
                            color: text_metrics[id].background,
                        });
                    }
                }
            }
        }
        pending.extend(document.nodes[id].children.iter().rev().copied());
    }
    let unsupported_rules = css
        .sheets
        .iter()
        .flat_map(|sheet| &sheet.rules.0)
        .filter(|rule| {
            !matches!(
                rule,
                lightningcss::rules::CssRule::Style(_)
                    | lightningcss::rules::CssRule::Import(_)
                    | lightningcss::rules::CssRule::Media(_)
            )
        })
        .count();
    let mut unused = alloc::collections::BTreeSet::new();
    let json = if diagnostics {
        for sheet in &css.sheets {
            unused_properties(&sheet.rules.0, &mut unused);
        }
        for (_, attribute) in &css.attributes {
            for property in attribute
                .declarations
                .declarations
                .iter()
                .chain(&attribute.declarations.important_declarations)
            {
                let name = property.property_id().name().to_owned();
                if !used_for_layout(&name) {
                    unused.insert(name);
                }
            }
        }
        serde_json::to_string_pretty(
        &serde_json::json!({"engine":"taffy","engine_version":"0.15.0","viewport":{"width":viewport.0,"height":viewport.1},"units":"layout units; non-text CSS lengths map 1:1 for compatibility; SVG uses the same numeric coordinates","page_text_scale":PAGE_TEXT_SCALE,"default_font_size":DEFAULT_FONT_SIZE,"coordinates":"absolute layout units; right/bottom are rectangle edges, not CSS insets","text_metrics":"synthetic monospace cell metrics: advance is font-size * 6/11; normal line height equals font size; default font size is 22 layout units after page text scaling; no font shaping","limitations":["opacity:0 suppresses subtree paint and visibility is inherited; partial-opacity groups and compositing are not implemented","text widths and wrapping use synthetic monospace metrics; no font shaping","dynamic pseudo-classes, pseudo-elements, :has and namespace selectors are not implemented","media conditions cover viewport sizes and orientation; device preference queries, calc, grid tracks, cascade layers and supports conditions are not implemented","inline flow uses approximate flex line boxes; nested inline boxes may wrap as a single item; no complete browser default stylesheet"],"unsupported_rule_count":unsupported_rules,"properties_not_used_for_layout":unused,"nodes":rows}),
    )?
    } else {
        String::new()
    };
    let extent = project_extent(tree.layout(root)?.size.height).max(viewport.1 as f32);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\"><rect width=\"100%\" height=\"100%\" fill=\"#{r:02x}{g:02x}{b:02x}\"/>",
        viewport.0, extent, viewport.0, extent
    );
    let line_rects = rectangles
        .iter()
        .map(|&(_, x, y, w, h)| [x, y, w, h])
        .collect();
    for (id, left, top, width, height) in rectangles {
        let _ = write!(
            svg,
            "<rect x=\"{left}\" y=\"{top}\" width=\"{width}\" height=\"{height}\" fill=\"none\" stroke=\"magenta\" stroke-width=\"1\"><title>node {id}</title></rect>"
        );
    }
    svg.push_str(&text_svg);
    svg.push_str("</svg>\n");
    phase("export");
    Ok(Export {
        json,
        svg,
        line_rects,
        background_quads,
        canvas_color,
        border_quads,
        button_boxes,
        line_extent: (viewport.0, extent.ceil() as u32),
        text_runs,
        image_boxes,
        link_boxes,
    })
}

fn project_position(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-MAX_DOCUMENT_EDGE, MAX_DOCUMENT_EDGE)
    } else {
        0.0
    }
}

fn project_extent(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, MAX_DOCUMENT_EDGE)
    } else {
        0.0
    }
}

/// Use CSS sizes first, HTML dimensions next, then the 300x150 replaced-element
/// fallback. These sizes never depend on downloaded pixels. Percentages remain
/// relative to the containing block. Percentage min/max constraints are projected
/// against the viewport for this bounded static image policy.
fn image_style(style: &mut Style, attrs: &[rp5::Attribute], viewport: (u32, u32)) {
    style.item_is_replaced = true;
    let attribute = |name: &str| {
        attrs
            .iter()
            .find(|attr| attr.namespace.is_none() && attr.name == name)
            .and_then(|attr| attr.value.trim().parse::<f32>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
    };
    let width = attribute("width");
    let height = attribute("height");
    if style.aspect_ratio.is_none() {
        style.aspect_ratio =
            width
                .zip(height)
                .filter(|&(w, h)| w > 0.0 && h > 0.0)
                .map(|(w, h)| {
                    (f64::from(w) / f64::from(h))
                        .clamp(1.0 / f64::from(MAX_IMAGE_EDGE), f64::from(MAX_IMAGE_EDGE))
                        as f32
                });
    }
    if style.size.width.is_auto() {
        if let Some(width) = width {
            style.size.width = Dimension::from_length(width);
        }
    }
    if style.size.height.is_auto() {
        if let Some(height) = height {
            style.size.height = Dimension::from_length(height);
        }
    }
    style.aspect_ratio = style
        .aspect_ratio
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0);
    let ratio = style
        .aspect_ratio
        .unwrap_or(2.0)
        .clamp(1.0 / MAX_IMAGE_EDGE, MAX_IMAGE_EDGE);
    if style.size.width.is_auto() && style.size.height.is_auto() {
        style.size.width = Dimension::from_length(300.0);
        style.size.height = Dimension::from_length((300.0 / ratio).min(MAX_IMAGE_EDGE));
    } else if style.size.width.is_auto() {
        if let Some(height) = style.size.height.into_option() {
            style.size.width = Dimension::from_length(
                (f64::from(height) * f64::from(ratio)).min(f64::from(f32::MAX)) as f32,
            );
        } else {
            style.aspect_ratio = Some(ratio);
        }
    } else if style.size.height.is_auto() {
        if let Some(width) = style.size.width.into_option() {
            style.size.height = Dimension::from_length(
                (f64::from(width) / f64::from(ratio)).min(f64::from(f32::MAX)) as f32,
            );
        } else {
            style.aspect_ratio = Some(ratio);
        }
    }
    // When both axes are known, fit the pair together to retain its ratio.
    if let Some((width, height)) = style
        .size
        .width
        .into_option()
        .zip(style.size.height.into_option())
    {
        let scale = (MAX_IMAGE_EDGE / width.max(height)).min(1.0);
        style.size.width = Dimension::from_length(width * scale);
        style.size.height = Dimension::from_length(height * scale);
    }
    for dimension in [&mut style.size.width, &mut style.size.height] {
        if let Some(value) = dimension.into_option() {
            *dimension = Dimension::from_length(value.clamp(0.0, MAX_IMAGE_EDGE));
        }
    }
    for (minimum, maximum, context) in [
        (
            &mut style.min_size.width,
            &mut style.max_size.width,
            viewport.0 as f32,
        ),
        (
            &mut style.min_size.height,
            &mut style.max_size.height,
            viewport.1 as f32,
        ),
    ] {
        *minimum = LengthPercentageAuto::from_length(
            minimum
                .resolve_to_option(context, |_, _| 0.0)
                .unwrap_or(0.0)
                .clamp(0.0, MAX_IMAGE_EDGE),
        );
        *maximum = LengthPercentageAuto::from_length(
            maximum
                .resolve_to_option(context, |_, _| 0.0)
                .unwrap_or(MAX_IMAGE_EDGE)
                .clamp(0.0, MAX_IMAGE_EDGE),
        );
    }
}

#[derive(Clone, Copy)]
struct TextMetrics {
    font_size: f32,
    line_height: LineHeightMetric,
    align: TextAlignMetric,
    whitespace: WhiteSpaceMetric,
    color: [u8; 4],
    background: [u8; 4],
    border_color: [u8; 4],
    border_radii: [[Radius; 2]; 4],
    // Zero opacity suppresses the whole subtree. Visibility is inherited but
    // a child may explicitly become visible; the two policies are distinct.
    opacity_zero: bool,
    visibility_hidden: bool,
}

struct TextMeasureContext {
    text: String,
    metrics: TextMetrics,
}

#[derive(Clone, Copy)]
enum LineHeightMetric {
    Normal,
    Number(f32),
    Absolute(f32),
}

impl LineHeightMetric {
    fn used(self, font_size: f32) -> f32 {
        match self {
            Self::Normal => font_size,
            Self::Number(value) => font_size * value,
            Self::Absolute(value) => value,
        }
    }
}

impl Default for TextMetrics {
    fn default() -> Self {
        Self {
            font_size: DEFAULT_FONT_SIZE,
            line_height: LineHeightMetric::Normal,
            align: TextAlignMetric::Start,
            whitespace: WhiteSpaceMetric::Normal,
            color: [0, 0, 0, 255],
            background: [0; 4],
            border_color: [0, 0, 0, 255],
            border_radii: [[Radius::default(); 2]; 4],
            opacity_zero: false,
            visibility_hidden: false,
        }
    }
}

impl TextMetrics {
    fn paints(self) -> bool {
        !self.opacity_zero && !self.visibility_hidden
    }
}

pub(super) fn estimate_text_width(text: &str, font_size: f32) -> f32 {
    (text.chars().count() as f64 * text_advance(font_size)) as f32
}

fn text_advance(font_size: f32) -> f64 {
    f64::from(font_size) * 6.0 / 11.0
}

fn apply_css<'a>(
    document: &Document,
    id: HtmlId,
    css: &'a Css,
    cascade: &CascadeIndex<'a>,
    dom: &MatchDom<'_>,
    scratch: &mut CascadeScratch<'a>,
    style: &mut Style,
    text_metrics: &mut TextMetrics,
    variables: &mut Variables,
    context: ResolveContext,
    inline: &mut bool,
    contents: &mut bool,
) {
    cascade.candidates(document, id, &mut scratch.candidates);
    let declarations = &mut scratch.declarations;
    declarations.clear();
    let mut order = 0;
    scratch.matched_rules.clear();
    for &candidate in &scratch.candidates {
        let entry = &cascade.selectors[candidate];
        if matches_components(
            document,
            dom,
            id,
            &entry.selector.components,
            &mut scratch.matching,
        ) {
            match scratch.matched_rules.last_mut() {
                Some((style, specificity)) if *style == entry.style => {
                    *specificity = (*specificity).max(entry.selector.specificity)
                }
                _ => scratch
                    .matched_rules
                    .push((entry.style, entry.selector.specificity)),
            }
        }
    }
    for &(style, specificity) in &scratch.matched_rules {
        let entry = &cascade.styles[style];
        for (important, properties) in [
            (false, &entry.rule.declarations.declarations),
            (true, &entry.rule.declarations.important_declarations),
        ] {
            for property in properties {
                declarations.push((important, false, specificity, order, property));
                order += 1;
            }
        }
    }
    if let Some(&attribute) = css.inline_by_node.get(&id) {
        let attribute = &css.attributes[attribute].1;
        for (important, properties) in [
            (false, &attribute.declarations.declarations),
            (true, &attribute.declarations.important_declarations),
        ] {
            for property in properties {
                declarations.push((important, true, 0, order, property));
                order += 1;
            }
        }
    }
    declarations.sort_by_key(|&(important, inline, specificity, order, _)| {
        (important, inline, specificity, order)
    });
    variables.cascade(declarations.iter().map(|declaration| declaration.4));
    let computed: Vec<alloc::borrow::Cow<'_, Property<'static>>> = declarations
        .iter()
        .filter_map(|declaration| match declaration.4 {
            Property::Custom(_) => None,
            Property::Unparsed(property) => {
                variables.property(property).map(alloc::borrow::Cow::Owned)
            }
            property => Some(alloc::borrow::Cow::Borrowed(property)),
        })
        .collect();
    let properties: Vec<_> = computed.iter().map(|property| property.as_ref()).collect();
    let mut positioned = false;
    let mut border_visible = [true; 4];
    let mut font_size_winner: Option<&CssFontSize> = None;
    let mut line_height_winner: Option<&CssLineHeight> = None;
    let mut color_winner = None;
    let mut background_winner = None;
    let mut border_color_winner = None;
    let mut opacity_winner = None;
    for property in &properties {
        if let Property::Position(value) = property {
            positioned = !matches!(value, lightningcss::properties::position::Position::Static);
        }
        match property {
            Property::Color(value) => color_winner = Some(value),
            // One closed loop uses one color: the cascaded top-border color.
            Property::Border(value) => border_color_winner = Some(&value.color),
            Property::BorderColor(value) => border_color_winner = Some(&value.top),
            Property::BorderTop(value) => border_color_winner = Some(&value.color),
            Property::BorderTopColor(value) => border_color_winner = Some(value),
            Property::BackgroundColor(value) => background_winner = Some(value),
            Property::Background(layers) => {
                background_winner = layers.last().map(|layer| &layer.color);
            }
            Property::FontSize(value) => font_size_winner = Some(value),
            Property::LineHeight(value) => line_height_winner = Some(value),
            Property::Opacity(value) => opacity_winner = Some(value.0),
            Property::Visibility(value) => {
                text_metrics.visibility_hidden = !matches!(
                    value,
                    lightningcss::properties::display::Visibility::Visible
                );
            }
            Property::Display(CssDisplay::Pair(value)) => {
                *inline = matches!(
                    value.outside,
                    lightningcss::properties::display::DisplayOutside::Inline
                );
                *contents = false;
            }
            Property::Display(CssDisplay::Keyword(DisplayKeyword::Contents)) => {
                *contents = true;
            }
            Property::TextAlign(value) => {
                use lightningcss::properties::text::TextAlign;
                text_metrics.align = match value {
                    TextAlign::Center => TextAlignMetric::Center,
                    TextAlign::Right | TextAlign::End => TextAlignMetric::End,
                    TextAlign::MatchParent => text_metrics.align,
                    _ => TextAlignMetric::Start,
                };
            }
            Property::WhiteSpace(value) => {
                use lightningcss::properties::text::WhiteSpace;
                text_metrics.whitespace = match value {
                    WhiteSpace::Pre => WhiteSpaceMetric::Pre,
                    WhiteSpace::PreWrap | WhiteSpace::BreakSpaces => WhiteSpaceMetric::PreWrap,
                    WhiteSpace::NoWrap => WhiteSpaceMetric::NoWrap,
                    _ => WhiteSpaceMetric::Normal,
                };
            }
            Property::Font(value) => {
                font_size_winner = Some(&value.size);
                line_height_winner = Some(&value.line_height);
            }
            _ => {}
        }
    }
    // Opacity is not inherited as a CSS value, but an ancestor's zero group
    // opacity cannot be undone by a child's opacity:1. Choose the winning own
    // value first so opacity:0 followed by opacity:1 remains visible.
    text_metrics.opacity_zero |= opacity_winner.is_some_and(|value| value <= 0.0);
    if let Some(value) = color_winner {
        if let Ok(lightningcss::values::color::CssColor::RGBA(value)) = value.to_rgb() {
            text_metrics.color = [value.red, value.green, value.blue, value.alpha];
        }
    }
    if let Some(value) = background_winner {
        use lightningcss::values::color::CssColor;
        if matches!(value, CssColor::CurrentColor) {
            text_metrics.background = text_metrics.color;
        } else if let Ok(CssColor::RGBA(value)) = value.to_rgb() {
            text_metrics.background = [value.red, value.green, value.blue, value.alpha];
        }
    }
    if let Some(value) = border_color_winner {
        use lightningcss::values::color::CssColor;
        if matches!(value, CssColor::CurrentColor) {
            text_metrics.border_color = text_metrics.color;
        } else if let Ok(CssColor::RGBA(value)) = value.to_rgb() {
            text_metrics.border_color = [value.red, value.green, value.blue, value.alpha];
        }
    }
    // Resolve every relative value against the inherited size, independent of
    // declaration iteration order. Font shorthand competes at its cascade position.
    if let Some(size) =
        font_size_winner.and_then(|value| font_size(value, text_metrics.font_size, context))
    {
        text_metrics.font_size = size;
    }
    if let Some(height) =
        line_height_winner.and_then(|value| line_height(value, text_metrics.font_size, context))
    {
        text_metrics.line_height = height;
    }
    let context = ResolveContext {
        font_size: text_metrics.font_size,
        ..context
    };
    for property in properties {
        use lightningcss::properties::border::LineStyle;
        let visible = |value: &LineStyle| !matches!(value, LineStyle::None | LineStyle::Hidden);
        match property {
            Property::BorderRadius(value, _) => {
                text_metrics.border_radii = [
                    &value.top_left,
                    &value.top_right,
                    &value.bottom_right,
                    &value.bottom_left,
                ]
                .map(|value| radius_pair(value, context));
            }
            Property::BorderTopLeftRadius(value, _) => {
                text_metrics.border_radii[0] = radius_pair(value, context)
            }
            Property::BorderTopRightRadius(value, _) => {
                text_metrics.border_radii[1] = radius_pair(value, context)
            }
            Property::BorderBottomRightRadius(value, _) => {
                text_metrics.border_radii[2] = radius_pair(value, context)
            }
            Property::BorderBottomLeftRadius(value, _) => {
                text_metrics.border_radii[3] = radius_pair(value, context)
            }
            Property::Border(value) => border_visible = [visible(&value.style); 4],
            Property::BorderTop(value) => border_visible[0] = visible(&value.style),
            Property::BorderRight(value) => border_visible[1] = visible(&value.style),
            Property::BorderBottom(value) => border_visible[2] = visible(&value.style),
            Property::BorderLeft(value) => border_visible[3] = visible(&value.style),
            Property::BorderStyle(value) => {
                border_visible = [
                    visible(&value.top),
                    visible(&value.right),
                    visible(&value.bottom),
                    visible(&value.left),
                ]
            }
            Property::BorderTopStyle(value) => border_visible[0] = visible(value),
            Property::BorderRightStyle(value) => border_visible[1] = visible(value),
            Property::BorderBottomStyle(value) => border_visible[2] = visible(value),
            Property::BorderLeftStyle(value) => border_visible[3] = visible(value),
            _ => {}
        }
        apply_property(style, property, context);
    }
    let zero = taffy::style::LengthPercentage::from_length(0.0);
    if !border_visible[0] {
        style.border.top = zero;
    }
    if !border_visible[1] {
        style.border.right = zero;
    }
    if !border_visible[2] {
        style.border.bottom = zero;
    }
    if !border_visible[3] {
        style.border.left = zero;
    }
    if !positioned {
        style.inset = Rect {
            top: LengthPercentageAuto::AUTO,
            left: LengthPercentageAuto::AUTO,
            bottom: LengthPercentageAuto::AUTO,
            right: LengthPercentageAuto::AUTO,
        };
    }
}

fn radius_pair(
    value: &lightningcss::values::size::Size2D<CssLengthPercentage>,
    context: ResolveContext,
) -> [Radius; 2] {
    [&value.0, &value.1].map(|value| match value {
        CssLengthPercentage::Dimension(value) => Radius {
            pixels: scalar(value, context).unwrap_or(0.0).max(0.0),
            fraction: 0.0,
        },
        CssLengthPercentage::Percentage(value) => Radius {
            pixels: 0.0,
            fraction: value.0.max(0.0),
        },
        _ => Radius::default(),
    })
}

/// Font-relative lengths already contain the page text scale through their
/// resolved em/rem. Scale independent lengths only at this text boundary;
/// ordinary CSS geometry continues to use `scalar` without magnification.
fn text_length(value: &LengthValue, context: ResolveContext) -> Option<f32> {
    let scale = if matches!(value, LengthValue::Em(_) | LengthValue::Rem(_)) {
        1.0
    } else {
        PAGE_TEXT_SCALE
    };
    scalar(value, context).map(|size| (size * scale).clamp(-MAX_LAYOUT_LENGTH, MAX_LAYOUT_LENGTH))
}

fn font_size(value: &CssFontSize, parent: f32, context: ResolveContext) -> Option<f32> {
    let size = match value {
        CssFontSize::Length(value) => match value {
            CssLengthPercentage::Dimension(value) => text_length(
                value,
                ResolveContext {
                    font_size: parent,
                    ..context
                },
            )?,
            CssLengthPercentage::Percentage(value) => parent * value.0,
            _ => return None,
        },
        CssFontSize::Absolute(value) => PAGE_TEXT_SCALE * match value {
            AbsoluteFontSize::XXSmall => 9.0,
            AbsoluteFontSize::XSmall => 10.0,
            AbsoluteFontSize::Small => 13.0,
            AbsoluteFontSize::Medium => 16.0,
            AbsoluteFontSize::Large => 18.0,
            AbsoluteFontSize::XLarge => 24.0,
            AbsoluteFontSize::XXLarge => 32.0,
            AbsoluteFontSize::XXXLarge => 48.0,
        },
        CssFontSize::Relative(value) => match value {
            RelativeFontSize::Larger => parent * 1.2,
            RelativeFontSize::Smaller => parent / 1.2,
        },
    };
    size.is_finite().then_some(size.clamp(0.0, 1_440.0))
}

fn line_height(
    value: &CssLineHeight,
    font_size: f32,
    context: ResolveContext,
) -> Option<LineHeightMetric> {
    let metric = match value {
        CssLineHeight::Normal => LineHeightMetric::Normal,
        CssLineHeight::Number(value) => LineHeightMetric::Number(value.clamp(0.0, 64.0)),
        CssLineHeight::Length(CssLengthPercentage::Dimension(value)) => {
            LineHeightMetric::Absolute(text_length(
                value,
                ResolveContext {
                    font_size,
                    ..context
                },
            )?)
        }
        CssLineHeight::Length(CssLengthPercentage::Percentage(value)) => {
            LineHeightMetric::Absolute((font_size * value.0).clamp(0.0, MAX_LAYOUT_LENGTH))
        }
        CssLineHeight::Length(_) => return None,
    };
    let valid = match metric {
        LineHeightMetric::Normal => true,
        LineHeightMetric::Number(value) | LineHeightMetric::Absolute(value) => value.is_finite(),
    };
    valid.then_some(metric)
}

#[derive(Clone, Copy)]
struct ResolveContext {
    viewport: (u32, u32),
    font_size: f32,
    root_font_size: f32,
}

fn scalar(value: &LengthValue, context: ResolveContext) -> Option<f32> {
    use LengthValue::*;
    let (w, h) = (context.viewport.0 as f32, context.viewport.1 as f32);
    let result = match value {
        Px(v) => *v,
        Em(v) => *v * context.font_size,
        Rem(v) => *v * context.root_font_size,
        Vw(v) | Svw(v) | Lvw(v) | Dvw(v) => *v * w / 100.0,
        Vh(v) | Svh(v) | Lvh(v) | Dvh(v) => *v * h / 100.0,
        Vmin(v) | Svmin(v) | Lvmin(v) | Dvmin(v) => *v * w.min(h) / 100.0,
        Vmax(v) | Svmax(v) | Lvmax(v) | Dvmax(v) => *v * w.max(h) / 100.0,
        In(v) => *v * 96.0,
        Cm(v) => *v * 96.0 / 2.54,
        Mm(v) => *v * 96.0 / 25.4,
        Pt(v) => *v * 96.0 / 72.0,
        Pc(v) => *v * 16.0,
        _ => return None,
    };
    result
        .is_finite()
        .then_some(result.clamp(-MAX_LAYOUT_LENGTH, MAX_LAYOUT_LENGTH))
}
fn raw_length(
    value: &CssLengthPercentage,
    context: ResolveContext,
) -> Option<taffy::style::LengthPercentage> {
    match value {
        CssLengthPercentage::Dimension(value) => {
            scalar(value, context).map(taffy::style::LengthPercentage::from_length)
        }
        CssLengthPercentage::Percentage(value) => Some(
            taffy::style::LengthPercentage::from_percent(value.0.clamp(-16.0, 16.0)),
        ),
        _ => None,
    }
}
fn length(
    value: &LengthPercentageOrAuto,
    context: ResolveContext,
) -> Option<taffy::style::LengthPercentage> {
    match value {
        LengthPercentageOrAuto::LengthPercentage(value) => raw_length(value, context),
        _ => None,
    }
}
fn auto(value: &LengthPercentageOrAuto, context: ResolveContext) -> Option<LengthPercentageAuto> {
    match value {
        LengthPercentageOrAuto::Auto => Some(LengthPercentageAuto::AUTO),
        LengthPercentageOrAuto::LengthPercentage(CssLengthPercentage::Dimension(value)) => {
            scalar(value, context).map(LengthPercentageAuto::from_length)
        }
        LengthPercentageOrAuto::LengthPercentage(CssLengthPercentage::Percentage(value)) => Some(
            LengthPercentageAuto::from_percent(value.0.clamp(-16.0, 16.0)),
        ),
        _ => None,
    }
}
fn dimension(value: &CssSize, context: ResolveContext) -> Option<Dimension> {
    match value {
        CssSize::Auto => Some(Dimension::AUTO),
        CssSize::LengthPercentage(CssLengthPercentage::Dimension(value)) => {
            scalar(value, context).map(Dimension::from_length)
        }
        CssSize::LengthPercentage(CssLengthPercentage::Percentage(value)) => {
            Some(Dimension::from_percent(value.0.clamp(0.0, 16.0)))
        }
        _ => None,
    }
}
fn minimum(value: &CssSize, context: ResolveContext) -> Option<LengthPercentageAuto> {
    match value {
        CssSize::Auto => Some(LengthPercentageAuto::AUTO),
        CssSize::LengthPercentage(value) => raw_length(value, context).map(Into::into),
        _ => None,
    }
}
fn maximum(
    value: &lightningcss::properties::size::MaxSize,
    context: ResolveContext,
) -> Option<LengthPercentageAuto> {
    use lightningcss::properties::size::MaxSize;
    match value {
        MaxSize::None => Some(LengthPercentageAuto::AUTO),
        MaxSize::LengthPercentage(value) => raw_length(value, context).map(Into::into),
        _ => None,
    }
}
fn border_width(
    value: &lightningcss::properties::border::BorderSideWidth,
    context: ResolveContext,
) -> Option<taffy::style::LengthPercentage> {
    use lightningcss::properties::border::BorderSideWidth::*;
    use lightningcss::values::length::Length;
    let width = match value {
        Thin => 1.0,
        Medium => 3.0,
        Thick => 5.0,
        Length(Length::Value(value)) => scalar(value, context)?,
        _ => return None,
    };
    Some(taffy::style::LengthPercentage::from_length(width))
}
fn border<const P: u8>(
    value: &lightningcss::properties::border::GenericBorder<
        lightningcss::properties::border::LineStyle,
        P,
    >,
    context: ResolveContext,
) -> Option<taffy::style::LengthPercentage> {
    use lightningcss::properties::border::LineStyle;
    if matches!(value.style, LineStyle::None | LineStyle::Hidden) {
        Some(taffy::style::LengthPercentage::from_length(0.0))
    } else {
        border_width(&value.width, context)
    }
}
fn apply_property(style: &mut Style, property: &Property<'_>, context: ResolveContext) {
    macro_rules! set {
        ($target:expr,$value:expr,$convert:ident) => {
            if let Some(value) = $convert($value, context) {
                $target = value;
            }
        };
    }
    match property {
        Property::AspectRatio(value) => {
            style.aspect_ratio = value
                .ratio
                .as_ref()
                .map(|ratio| ratio.0 / ratio.1)
                .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
                .map(|ratio| ratio.clamp(1.0 / MAX_IMAGE_EDGE, MAX_IMAGE_EDGE));
        }
        Property::MaxWidth(value) => set!(style.max_size.width, value, maximum),
        Property::MaxHeight(value) => set!(style.max_size.height, value, maximum),
        Property::BoxSizing(value, _) => {
            style.box_sizing = match value {
                lightningcss::properties::size::BoxSizing::BorderBox => BoxSizing::BorderBox,
                _ => BoxSizing::ContentBox,
            }
        }
        Property::Border(value) => {
            if let Some(width) = border(value, context) {
                style.border = Rect {
                    top: width,
                    left: width,
                    bottom: width,
                    right: width,
                };
            }
        }
        Property::BorderTop(value) => set!(style.border.top, value, border),
        Property::BorderLeft(value) => set!(style.border.left, value, border),
        Property::BorderBottom(value) => set!(style.border.bottom, value, border),
        Property::BorderRight(value) => set!(style.border.right, value, border),
        Property::BorderTopWidth(value) => set!(style.border.top, value, border_width),
        Property::BorderLeftWidth(value) => set!(style.border.left, value, border_width),
        Property::BorderBottomWidth(value) => set!(style.border.bottom, value, border_width),
        Property::BorderRightWidth(value) => set!(style.border.right, value, border_width),
        Property::BorderWidth(value) => {
            set!(style.border.top, &value.top, border_width);
            set!(style.border.left, &value.left, border_width);
            set!(style.border.bottom, &value.bottom, border_width);
            set!(style.border.right, &value.right, border_width);
        }
        Property::Width(value) => set!(style.size.width, value, dimension),
        Property::Height(value) => set!(style.size.height, value, dimension),
        Property::MinWidth(value) => set!(style.min_size.width, value, minimum),
        Property::MinHeight(value) => set!(style.min_size.height, value, minimum),
        Property::Margin(value) => {
            set!(style.margin.top, &value.top, auto);
            set!(style.margin.left, &value.left, auto);
            set!(style.margin.bottom, &value.bottom, auto);
            set!(style.margin.right, &value.right, auto);
        }
        Property::MarginTop(value) => set!(style.margin.top, value, auto),
        Property::MarginLeft(value) => set!(style.margin.left, value, auto),
        Property::MarginBottom(value) => set!(style.margin.bottom, value, auto),
        Property::MarginRight(value) => set!(style.margin.right, value, auto),
        Property::Padding(value) => {
            set!(style.padding.top, &value.top, length);
            set!(style.padding.left, &value.left, length);
            set!(style.padding.bottom, &value.bottom, length);
            set!(style.padding.right, &value.right, length);
        }
        Property::PaddingTop(value) => set!(style.padding.top, value, length),
        Property::PaddingLeft(value) => set!(style.padding.left, value, length),
        Property::PaddingBottom(value) => set!(style.padding.bottom, value, length),
        Property::PaddingRight(value) => set!(style.padding.right, value, length),
        Property::Top(value) => set!(style.inset.top, value, auto),
        Property::Left(value) => set!(style.inset.left, value, auto),
        Property::Bottom(value) => set!(style.inset.bottom, value, auto),
        Property::Right(value) => set!(style.inset.right, value, auto),
        Property::Display(CssDisplay::Keyword(DisplayKeyword::None)) => {
            style.display = Display::None
        }
        Property::Display(CssDisplay::Pair(value)) => {
            style.display = match value.inside {
                DisplayInside::Flex(_) => Display::Flex,
                DisplayInside::Grid => Display::Grid,
                _ => Display::Block,
            }
        }
        Property::Position(value) => {
            style.position = match value {
                lightningcss::properties::position::Position::Absolute
                | lightningcss::properties::position::Position::Fixed => Position::Absolute,
                _ => Position::Relative,
            }
        }
        Property::FlexGrow(value, _) => style.flex_grow = value.clamp(0.0, 1000.0),
        Property::FlexShrink(value, _) => style.flex_shrink = value.clamp(0.0, 1000.0),
        Property::FlexDirection(value, _) => {
            style.flex_direction = match value {
                lightningcss::properties::flex::FlexDirection::Row => FlexDirection::Row,
                lightningcss::properties::flex::FlexDirection::Column => FlexDirection::Column,
                lightningcss::properties::flex::FlexDirection::RowReverse => {
                    FlexDirection::RowReverse
                }
                lightningcss::properties::flex::FlexDirection::ColumnReverse => {
                    FlexDirection::ColumnReverse
                }
            }
        }
        _ => {}
    }
}

type Declaration<'a> = (bool, bool, u32, usize, &'a Property<'static>);

struct IndexedStyle<'a> {
    rule: &'a lightningcss::rules::style::StyleRule<'static>,
}

struct IndexedSelector<'a> {
    style: usize,
    selector: &'a super::css::PreparedSelector,
}

/// A viewport's active cascade, in HTML/import order. Buckets contain only
/// rules that could match a node, rather than all rules in all stylesheets.
#[derive(Default)]
struct CascadeIndex<'a> {
    styles: Vec<IndexedStyle<'a>>,
    selectors: Vec<IndexedSelector<'a>>,
    ids: HashMap<&'a str, Vec<usize>>,
    classes: HashMap<&'a str, Vec<usize>>,
    tags: HashMap<&'a str, Vec<usize>>,
    attributes: HashMap<&'a str, Vec<usize>>,
    universal: Vec<usize>,
}

impl<'a> CascadeIndex<'a> {
    fn new(css: &'a Css, context: ResolveContext) -> Self {
        let mut index = Self::default();
        let mut active = Vec::new();
        if css.roots.is_empty() {
            for sheet in 0..css.sheets.len() {
                index.add_sheet(css, sheet, context, &mut active);
            }
        } else {
            for root in &css.roots {
                if root
                    .media
                    .as_ref()
                    .is_some_and(|media| !media_matches(media, context))
                {
                    continue;
                }
                let sheet = match &root.source {
                    super::css::SheetReference::Embedded(sheet) => Some(*sheet),
                    super::css::SheetReference::External(url) => {
                        css.external_sheets.get(url).copied()
                    }
                };
                if let Some(sheet) = sheet {
                    index.add_sheet(css, sheet, context, &mut active);
                }
            }
        }
        index
    }

    fn add_sheet(
        &mut self,
        css: &'a Css,
        sheet: usize,
        context: ResolveContext,
        active: &mut Vec<usize>,
    ) {
        if active.contains(&sheet) {
            return;
        }
        let Some(prepared) = css.prepared_sheets.get(sheet) else {
            return;
        };
        active.push(sheet);
        self.add_rules(css, &css.sheets[sheet].rules.0, prepared, context, active);
        active.pop();
    }

    fn add_rules(
        &mut self,
        css: &'a Css,
        rules: &'a [lightningcss::rules::CssRule<'static>],
        prepared: &'a super::css::PreparedRules,
        context: ResolveContext,
        active: &mut Vec<usize>,
    ) {
        use super::css::{PreparedRule, SelectorKey};
        use lightningcss::rules::CssRule;
        for entry in &prepared.rules {
            match entry {
                PreparedRule::Style { index, selectors } => {
                    let CssRule::Style(rule) = &rules[*index] else {
                        unreachable!()
                    };
                    let style = self.styles.len();
                    self.styles.push(IndexedStyle { rule });
                    for selector in selectors {
                        if selector.never_matches {
                            continue;
                        }
                        let id = self.selectors.len();
                        self.selectors.push(IndexedSelector { style, selector });
                        match &selector.key {
                            Some(SelectorKey::Id(key)) => self.ids.entry(key).or_default().push(id),
                            Some(SelectorKey::Class(key)) => {
                                self.classes.entry(key).or_default().push(id)
                            }
                            Some(SelectorKey::Tag(key)) => {
                                self.tags.entry(key).or_default().push(id)
                            }
                            Some(SelectorKey::Attribute(key)) => {
                                self.attributes.entry(key).or_default().push(id)
                            }
                            None => self.universal.push(id),
                        }
                    }
                }
                PreparedRule::Media { index, children } => {
                    let CssRule::Media(rule) = &rules[*index] else {
                        unreachable!()
                    };
                    if media_matches(&rule.query, context) {
                        self.add_rules(css, &rule.rules.0, children, context, active);
                    }
                }
                PreparedRule::Import { index, url } => {
                    let CssRule::Import(rule) = &rules[*index] else {
                        unreachable!()
                    };
                    if rule.supports.is_none()
                        && rule.layer.is_none()
                        && media_matches(&rule.media, context)
                    {
                        if let Some(sheet) = url
                            .as_ref()
                            .and_then(|url| css.external_sheets.get(url))
                            .copied()
                        {
                            self.add_sheet(css, sheet, context, active);
                        }
                    }
                }
            }
        }
    }

    fn candidates(&self, document: &Document, id: HtmlId, out: &mut Vec<usize>) {
        out.clear();
        out.extend_from_slice(&self.universal);
        if let Some((tag, _, attrs)) = document.element(id) {
            if let Some(bucket) = self.tags.get(tag) {
                out.extend_from_slice(bucket);
            }
            for attribute in attrs
                .iter()
                .filter(|attribute| attribute.namespace.is_none())
            {
                if let Some(bucket) = self.attributes.get(attribute.name.as_str()) {
                    out.extend_from_slice(bucket);
                }
                if attribute.name == "id" {
                    if let Some(bucket) = self.ids.get(attribute.value.as_str()) {
                        out.extend_from_slice(bucket);
                    }
                } else if attribute.name == "class" {
                    for class in attribute.value.split_ascii_whitespace() {
                        if let Some(bucket) = self.classes.get(class) {
                            out.extend_from_slice(bucket);
                        }
                    }
                }
            }
        }
        // One rule can be reached through several matching selectors/keys.
        // Restore source order and apply its declarations only once.
        out.sort_unstable();
        out.dedup();
    }
}

#[derive(Default)]
struct CascadeScratch<'a> {
    candidates: Vec<usize>,
    matching: Vec<(HtmlId, usize)>,
    matched_rules: Vec<(usize, u32)>,
    declarations: Vec<Declaration<'a>>,
}

fn media_matches(
    media: &lightningcss::media_query::MediaList<'_>,
    context: ResolveContext,
) -> bool {
    use lightningcss::media_query::{MediaType, Qualifier};
    media.media_queries.is_empty()
        || media.media_queries.iter().any(|query| {
            let result = matches!(query.media_type, MediaType::All | MediaType::Screen)
                && query
                    .condition
                    .as_ref()
                    .is_none_or(|condition| media_condition(condition, context));
            if matches!(query.qualifier, Some(Qualifier::Not)) {
                !result
            } else {
                result
            }
        })
}
fn media_condition(
    condition: &lightningcss::media_query::MediaCondition<'_>,
    context: ResolveContext,
) -> bool {
    use lightningcss::media_query::{MediaCondition, Operator};
    match condition {
        MediaCondition::Not(condition) => !media_condition(condition, context),
        MediaCondition::Operation {
            operator,
            conditions,
        } => match operator {
            Operator::And => conditions
                .iter()
                .all(|condition| media_condition(condition, context)),
            Operator::Or => conditions
                .iter()
                .any(|condition| media_condition(condition, context)),
        },
        MediaCondition::Feature(feature) => media_feature(feature, context),
        _ => false,
    }
}
fn media_feature(
    feature: &lightningcss::media_query::MediaFeature<'_>,
    context: ResolveContext,
) -> bool {
    use lightningcss::media_query::{
        MediaFeatureComparison as Comparison, MediaFeatureId as Id, MediaFeatureName as Name,
        MediaFeatureValue as Value, QueryFeature,
    };
    let compare = |actual: f32, operator: Comparison, value: &Value<'_>| {
        let wanted = match value {
            Value::Length(lightningcss::values::length::Length::Value(value)) => scalar(
                value,
                ResolveContext {
                    // Media queries measure the viewport in unscaled CSS
                    // units; page text magnification does not resize it.
                    font_size: INITIAL_FONT_SIZE,
                    root_font_size: INITIAL_FONT_SIZE,
                    ..context
                },
            ),
            Value::Number(value) => Some(*value),
            Value::Ratio(value) => Some(value.0 / value.1),
            _ => None,
        };
        wanted.is_some_and(|wanted| match operator {
            Comparison::Equal => actual == wanted,
            Comparison::LessThan => actual < wanted,
            Comparison::LessThanEqual => actual <= wanted,
            Comparison::GreaterThan => actual > wanted,
            Comparison::GreaterThanEqual => actual >= wanted,
        })
    };
    let actual = |name: &Name<'_, Id>| match name {
        Name::Standard(Id::Width) => Some(context.viewport.0 as f32),
        Name::Standard(Id::Height) => Some(context.viewport.1 as f32),
        Name::Standard(Id::AspectRatio) => {
            Some(context.viewport.0 as f32 / context.viewport.1.max(1) as f32)
        }
        _ => None,
    };
    match feature {
        QueryFeature::Plain {
            name: Name::Standard(Id::Orientation),
            value: Value::Ident(value),
        } => match value.as_ref() {
            "landscape" => context.viewport.0 > context.viewport.1,
            "portrait" => context.viewport.0 <= context.viewport.1,
            _ => false,
        },
        QueryFeature::Plain { name, value } => {
            actual(name).is_some_and(|value_actual| compare(value_actual, Comparison::Equal, value))
        }
        QueryFeature::Range {
            name,
            operator,
            value,
        } => actual(name).is_some_and(|value_actual| compare(value_actual, *operator, value)),
        QueryFeature::Interval {
            name,
            start,
            start_operator,
            end,
            end_operator,
        } => actual(name).is_some_and(|value_actual| {
            let inverse = match start_operator {
                Comparison::LessThan => Comparison::GreaterThan,
                Comparison::LessThanEqual => Comparison::GreaterThanEqual,
                Comparison::GreaterThan => Comparison::LessThan,
                Comparison::GreaterThanEqual => Comparison::LessThanEqual,
                Comparison::Equal => Comparison::Equal,
            };
            compare(value_actual, inverse, start) && compare(value_actual, *end_operator, end)
        }),
        QueryFeature::Boolean { name } => actual(name).is_some_and(|value| value != 0.0),
    }
}

#[derive(Clone, Copy)]
enum TextAlignMetric {
    Start,
    Center,
    End,
}
impl TextAlignMetric {
    fn justify(self) -> JustifyContent {
        match self {
            Self::Start => JustifyContent::START,
            Self::Center => JustifyContent::CENTER,
            Self::End => JustifyContent::END,
        }
    }
}
#[derive(Clone, Copy)]
enum WhiteSpaceMetric {
    Normal,
    NoWrap,
    Pre,
    PreWrap,
}
fn default_inline(tag: &str) -> bool {
    matches!(
        tag,
        "a" | "abbr"
            | "b"
            | "bdi"
            | "bdo"
            | "br"
            | "button"
            | "cite"
            | "code"
            | "del"
            | "em"
            | "i"
            | "img"
            | "input"
            | "ins"
            | "kbd"
            | "label"
            | "mark"
            | "q"
            | "s"
            | "samp"
            | "select"
            | "small"
            | "span"
            | "strong"
            | "sub"
            | "sup"
            | "textarea"
            | "time"
            | "u"
            | "var"
    )
}
fn text_chunks(text: &str, whitespace: WhiteSpaceMetric) -> Vec<String> {
    if matches!(
        whitespace,
        WhiteSpaceMetric::Pre | WhiteSpaceMetric::PreWrap
    ) {
        return if text.is_empty() {
            vec![]
        } else {
            vec![text.into()]
        };
    }
    let words: Vec<_> = text
        .split(is_css_space)
        .filter(|word| !word.is_empty())
        .collect();
    if matches!(whitespace, WhiteSpaceMetric::NoWrap) {
        return if words.is_empty() {
            vec![]
        } else {
            vec![words.join(" ")]
        };
    }
    if words.is_empty() {
        return if text.is_empty() {
            vec![]
        } else {
            vec![" ".into()]
        };
    }
    let mut result = Vec::new();
    if text.starts_with(is_css_space) {
        result.push(" ".into());
    }
    for (index, word) in words.iter().enumerate() {
        let mut word = word.to_string();
        if index + 1 < words.len() || text.ends_with(is_css_space) {
            word.push(' ');
        }
        result.push(word);
    }
    result
}
fn flush_inline_run(
    tree: &mut TaffyTree<TextMeasureContext>,
    children: &mut Vec<NodeId>,
    run: &mut Vec<NodeId>,
    metrics: TextMetrics,
    full_width: bool,
) -> Result<(), taffy::TaffyError> {
    if run.is_empty() {
        return Ok(());
    }
    let style = Style {
        display: Display::Flex,
        flex_wrap: if matches!(
            metrics.whitespace,
            WhiteSpaceMetric::NoWrap | WhiteSpaceMetric::Pre
        ) {
            FlexWrap::NoWrap
        } else {
            FlexWrap::Wrap
        },
        align_items: AlignItems::START,
        align_content: AlignContent::START,
        justify_content: metrics.align.justify(),
        size: Size {
            width: if full_width {
                Dimension::from_percent(1.0)
            } else {
                Dimension::AUTO
            },
            height: Dimension::AUTO,
        },
        ..Style::default()
    };
    children.push(tree.new_with_children(style, run)?);
    run.clear();
    Ok(())
}
#[derive(Clone, Copy)]
struct BoxRect {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}
impl BoxRect {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({"left":self.left,"top":self.top,"right":self.left+self.width,"bottom":self.top+self.height,"width":self.width,"height":self.height})
    }
    fn bounds(rects: &[Self]) -> Option<Self> {
        let first = *rects.first()?;
        Some(rects.iter().skip(1).fold(first, |a, b| {
            let left = a.left.min(b.left);
            let top = a.top.min(b.top);
            Self {
                left,
                top,
                width: (a.left + a.width).max(b.left + b.width) - left,
                height: (a.top + a.height).max(b.top + b.height) - top,
            }
        }))
    }
}
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn visual_lines(context: &TextMeasureContext, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for_each_visual_line(
        &context.text,
        context.metrics.whitespace,
        width,
        context.metrics.font_size,
        |line| lines.push(line.to_owned()),
    );
    lines
}

fn visual_line_count(
    text: &str,
    whitespace: WhiteSpaceMetric,
    width: f32,
    font_size: f32,
) -> usize {
    let mut count = 0;
    for_each_visual_line(text, whitespace, width, font_size, |_| count += 1);
    count
}

// Measurement and emission must wrap the same Unicode characters. In
// particular, pre-wrap spaces consume columns just like visible letters.
// Walking borrowed slices keeps Taffy's repeated measurements allocation-free.
fn for_each_visual_line(
    text: &str,
    whitespace: WhiteSpaceMetric,
    width: f32,
    font_size: f32,
    mut emit: impl FnMut(&str),
) {
    if !font_size.is_finite() || font_size <= 0.0 {
        return;
    }
    let text = if matches!(whitespace, WhiteSpaceMetric::Normal) {
        // The leaf's natural width still reserves its inter-word spacing.
        // A trailing collapsed space must not create a second visual line.
        text.trim_end_matches(is_css_space)
    } else {
        text
    };
    if text.is_empty()
        && !matches!(
            whitespace,
            WhiteSpaceMetric::Pre | WhiteSpaceMetric::PreWrap
        )
    {
        return;
    }
    let advance = text_advance(font_size);
    let width = f64::from(width);
    // Taffy stores measured widths as f32. Allow only that conversion's
    // rounding error; an exact character fit must not lose a whole column.
    let rounding_slack = width.abs() * f64::from(f32::EPSILON);
    let columns = if advance > 0.0 {
        ((width + rounding_slack) / advance).floor().max(1.0) as usize
    } else {
        usize::MAX
    };
    let nowrap = matches!(whitespace, WhiteSpaceMetric::Pre | WhiteSpaceMetric::NoWrap);
    for line in text.split('\n') {
        if nowrap || line.is_empty() {
            emit(line);
            continue;
        }
        let mut start = 0;
        let mut used = 0;
        for (index, _) in line.char_indices() {
            if used == columns {
                emit(&line[start..index]);
                start = index;
                used = 0;
            }
            used += 1;
        }
        emit(&line[start..]);
    }
}

fn used_for_layout(name: &str) -> bool {
    if name.starts_with("--") {
        return true;
    }
    matches!(
        name,
        "width"
            | "height"
            | "aspect-ratio"
            | "min-width"
            | "min-height"
            | "max-width"
            | "max-height"
            | "box-sizing"
            | "margin"
            | "margin-top"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "padding"
            | "padding-top"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "border"
            | "border-top"
            | "border-right"
            | "border-bottom"
            | "border-left"
            | "border-width"
            | "border-style"
            | "border-top-width"
            | "border-right-width"
            | "border-bottom-width"
            | "border-left-width"
            | "border-top-style"
            | "border-right-style"
            | "border-bottom-style"
            | "border-left-style"
            | "position"
            | "top"
            | "right"
            | "bottom"
            | "left"
            | "display"
            | "font"
            | "font-size"
            | "color"
            | "background-color"
            | "background"
            | "border-color"
            | "border-top-color"
            | "border-radius"
            | "border-top-left-radius"
            | "border-top-right-radius"
            | "border-bottom-right-radius"
            | "border-bottom-left-radius"
            | "opacity"
            | "visibility"
            | "line-height"
            | "text-align"
            | "white-space"
            | "flex-grow"
            | "flex-shrink"
            | "flex-direction"
    )
}
fn unused_properties(
    rules: &[lightningcss::rules::CssRule<'static>],
    unused: &mut alloc::collections::BTreeSet<String>,
) {
    use lightningcss::rules::CssRule;
    for rule in rules {
        match rule {
            CssRule::Style(rule) => {
                for property in rule
                    .declarations
                    .declarations
                    .iter()
                    .chain(&rule.declarations.important_declarations)
                {
                    let name = property.property_id().name().to_owned();
                    if !used_for_layout(&name) {
                        unused.insert(name);
                    }
                }
                unused_properties(&rule.rules.0, unused);
            }
            CssRule::Media(rule) => unused_properties(&rule.rules.0, unused),
            _ => {}
        }
    }
}

fn is_css_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r' | '\u{000c}')
}

#[cfg(test)]
mod cascade_index_tests {
    use super::*;
    use static_self::IntoOwned;
    #[test]
    fn unrelated_rules_do_not_increase_a_nodes_candidate_set() {
        let document = rp5::parse("<div id='box' class='target target' data-size='wide'></div>");
        let node = document
            .nodes
            .iter()
            .enumerate()
            .find_map(|(id, _)| {
                document.element(id).and_then(|(_, _, attrs)| {
                    attrs
                        .iter()
                        .any(|attr| attr.name == "id" && attr.value == "box")
                        .then_some(id)
                })
            })
            .unwrap();
        let mut source =
            String::from(".target,#box{width:83px}[data-size]{height:47px}*{margin:0}");
        for i in 0..2000 {
            write!(source, ".unrelated{i}{{width:42px}}").unwrap();
        }
        let mut css = Css::default();
        css.add_sheet(
            lightningcss::stylesheet::StyleSheet::parse(&source, Default::default())
                .unwrap()
                .into_owned(),
        );
        let index = CascadeIndex::new(
            &css,
            ResolveContext {
                viewport: (1280, 720),
                font_size: 11.0,
                root_font_size: 11.0,
            },
        );
        let mut candidates = Vec::new();
        index.candidates(&document, node, &mut candidates);
        assert_eq!(index.styles.len(), 2003);
        let mut styles: Vec<_> = candidates
            .iter()
            .map(|&id| index.selectors[id].style)
            .collect();
        styles.dedup();
        assert_eq!(styles, vec![0, 1, 2]);
    }
}
