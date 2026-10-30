//! Image references and frozen boxes only. Fetching, caching, decoding and GPU
//! upload belong to the kernel image service; Hull never receives image bytes.
use std::collections::HashMap;
use std::time::{Duration, Instant};
use trueos::logl;
use trueos::ui4_scene::{
    HULL_IMAGE_FAILED, HULL_IMAGE_READY, hull_image_release, hull_image_request, hull_image_reset,
    hull_image_status,
};
use trueos::ui4_winit::{Error, SceneTarget, SpriteCorner, SpriteQuad};
use url::Url;

use super::{layout::ImageBox, viewport::Viewport};

const MAX_ASSETS: usize = 8;
const POLLS_PER_TICK: usize = 2;
const MAX_VISIBLE_BOXES: usize = 2048;
// Allow all eight tickets to queue behind the kernel's two fetch workers.
// The kernel owns per-stage deadlines; this is only an orphan/admission guard.
const IMAGE_DEADLINE: Duration = Duration::from_secs(90);

#[derive(Default)]
pub(super) struct Images {
    window: Option<u32>,
    generation: u64,
    root: String,
    host: String,
    boxes: Vec<(ImageBox, Option<usize>)>,
    assets: Vec<Asset>,
    cursor: usize,
    published: bool,
    reset_pending: bool,
    paint_disabled: bool,
}

struct Asset {
    url: String,
    size: (u32, u32),
    ticket: Option<u32>,
    sprite: Option<u32>,
    started: Option<Instant>,
    finished: bool,
}

impl Images {
    pub fn reset(&mut self, window: u32, generation: u64) {
        self.release();
        *self = Self::default();
        self.window = Some(window);
        self.generation = generation;
        self.reset_pending = true;
        self.poll();
    }

    /// Collect all paintable boxes, including those with no source or over the
    /// admission limit. Such boxes retain a placeholder and their flow space.
    pub fn collect(&mut self, document: &rp5::Document, page_url: &str, boxes: Vec<ImageBox>) {
        self.root = super::snapshot::app_root();
        self.host = Url::parse(page_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        let base = document_base(document, page_url);
        let mut admitted = HashMap::new();
        for image in boxes {
            let url = base.as_ref().and_then(|base| {
                let (_, rp5::Namespace::Html, attrs) = document.element(image.node)? else {
                    return None;
                };
                let src = attribute(attrs, "src")?.trim();
                if src.is_empty() || src.len() > 4096 {
                    return None;
                }
                let mut url = base.join(src).ok()?;
                if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some()
                {
                    return None;
                }
                url.set_fragment(None);
                Some(url.to_string())
            });
            let size = request_size(image.rect);
            let asset = url.and_then(|url| {
                if let Some(&index) = admitted.get(&url) {
                    return Some(index);
                }
                let size = size?;
                if self.assets.len() == MAX_ASSETS {
                    return None;
                }
                let index = self.assets.len();
                admitted.insert(url.clone(), index);
                self.assets.push(Asset {
                    url,
                    size,
                    ticket: None,
                    sprite: None,
                    started: None,
                    finished: false,
                });
                Some(index)
            });
            if let Some((index, size)) = asset.zip(size) {
                let current = &mut self.assets[index].size;
                current.0 = current.0.max(size.0);
                current.1 = current.1.max(size.1);
            }
            self.boxes.push((image, asset));
        }
    }

    /// Only a successful publication of this navigation's layout opens admission.
    pub fn published(&mut self) {
        self.published = true;
    }

    pub fn poll(&mut self) -> bool {
        let Some(window) = self.window else {
            return false;
        };
        if self.reset_pending {
            match hull_image_reset(window, self.generation) {
                Ok(()) => self.reset_pending = false,
                Err(Error::Busy) => return false,
                Err(error) => {
                    report(format_args!("generation reset: {error:?}"));
                    self.window = None;
                    return false;
                }
            }
        }
        if !self.published {
            return false;
        }
        // At most one request and two status reads per event-loop turn.
        if let Some(asset) = self
            .assets
            .iter_mut()
            .find(|asset| !asset.finished && asset.ticket.is_none())
        {
            let started = asset.started.get_or_insert_with(Instant::now);
            if started.elapsed() > IMAGE_DEADLINE {
                asset.finished = true;
            } else {
                match hull_image_request(
                    window,
                    &asset.url,
                    &self.root,
                    &self.host,
                    asset.size.0,
                    asset.size.1,
                    self.generation,
                ) {
                    Ok(ticket) => asset.ticket = Some(ticket),
                    Err(Error::Busy) => {}
                    Err(error) => {
                        asset.finished = true;
                        report(format_args!("request failed: {error:?}"));
                    }
                }
            }
        }
        let mut changed = false;
        for _ in 0..POLLS_PER_TICK.min(self.assets.len()) {
            let index = self.cursor % self.assets.len();
            self.cursor = (index + 1) % self.assets.len();
            let asset = &mut self.assets[index];
            if asset.finished {
                if asset.sprite.is_none() {
                    release_ticket(window, asset);
                }
                continue;
            }
            let Some(ticket) = asset.ticket else {
                continue;
            };
            let expired = asset
                .started
                .is_some_and(|started| started.elapsed() > IMAGE_DEADLINE);
            match hull_image_status(window, ticket) {
                Ok(info) if info.generation != self.generation => asset.finished = true,
                Ok(info) if info.state == HULL_IMAGE_READY && info.sprite_id != 0 => {
                    asset.sprite = Some(info.sprite_id);
                    asset.finished = true;
                    changed = true;
                }
                Ok(info) if info.state == HULL_IMAGE_FAILED => {
                    asset.finished = true;
                    report(format_args!("asset failed: {}", info.error));
                }
                Ok(_) | Err(Error::Busy) if !expired => {}
                Ok(_) | Err(_) => asset.finished = true,
            }
            if asset.finished && asset.sprite.is_none() {
                release_ticket(window, asset);
            }
        }
        changed
    }

    /// One sprite submission overlays the native diagnostic boxes before text.
    /// Its ordered commands preserve image alpha and overlapping paint order.
    pub fn draw(&mut self, viewport: &Viewport, target: &mut SceneTarget) -> bool {
        if !self.published || self.paint_disabled {
            return false;
        }
        let mut quads = Vec::new();
        for &(image, asset) in &self.boxes {
            let Some((rect, uv)) = viewport.clip_image(image.rect) else {
                continue;
            };
            let sprite = asset.and_then(|index| self.assets[index].sprite);
            quads.push(quad(
                sprite.unwrap_or(0),
                rect,
                uv,
                if sprite.is_some() {
                    u32::MAX
                } else {
                    u32::from_le_bytes([232, 235, 239, 255])
                },
            ));
            if quads.len() == MAX_VISIBLE_BOXES {
                break;
            }
        }
        if quads.is_empty() {
            return false;
        }
        match target.draw_sprite_quads(&quads) {
            Ok(()) => false,
            Err(error) => {
                // Even Busy may have canceled the current write lease. Disable
                // this navigation's image paint and let Hull recover a text frame.
                self.paint_disabled = true;
                for asset in &mut self.assets {
                    asset.finished = true;
                    asset.sprite = None;
                    release_ticket(target.render_target(), asset);
                }
                report(format_args!("sprite draw failed: {error:?}"));
                true
            }
        }
    }

    fn release(&mut self) {
        if let Some(window) = self.window {
            for asset in &mut self.assets {
                if let Some(ticket) = asset.ticket.take() {
                    let _ = hull_image_release(window, ticket);
                }
            }
        }
    }
}

impl Drop for Images {
    fn drop(&mut self) {
        self.release();
    }
}

/// Recover either a canceled lease or the still-owned lease left by a rejected
/// draw. Busy preserves quarantine; only the following text/publication checks
/// decide whether an existing lease's GPU work has retired.
pub(super) fn recover_frame(target: &mut SceneTarget) -> Result<(), Error> {
    match target.begin(u32::MAX) {
        Ok(()) | Err(Error::InvalidState) => Ok(()),
        Err(error) => Err(error),
    }
}

fn release_ticket(window: u32, asset: &mut Asset) {
    if let Some(ticket) = asset.ticket {
        if !matches!(hull_image_release(window, ticket), Err(Error::Busy)) {
            asset.ticket = None;
        }
    }
}

fn request_size([_, _, width, height]: [f32; 4]) -> Option<(u32, u32)> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some((
        width.ceil().min(super::layout::MAX_IMAGE_EDGE) as u32,
        height.ceil().min(super::layout::MAX_IMAGE_EDGE) as u32,
    ))
}

fn document_base(document: &rp5::Document, page_url: &str) -> Option<Url> {
    let mut base = Url::parse(page_url).ok()?;
    let mut pending = vec![document.root];
    while let Some(id) = pending.pop() {
        if let Some(("base", rp5::Namespace::Html, attrs)) = document.element(id) {
            if let Some(href) = attribute(attrs, "href") {
                if let Ok(resolved) = base.join(href) {
                    base = resolved;
                }
                break;
            }
        }
        pending.extend(document.nodes[id].children.iter().rev().copied());
    }
    Some(base)
}

fn attribute<'a>(attrs: &'a [rp5::Attribute], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|attr| attr.namespace.is_none() && attr.name == name)
        .map(|attr| attr.value.as_str())
}

fn quad(
    sprite_id: u32,
    [x, y, width, height]: [f32; 4],
    [u0, v0, u1, v1]: [f32; 4],
    color_rgba: u32,
) -> SpriteQuad {
    SpriteQuad {
        sprite_id,
        c0: SpriteCorner { x, y, u: u0, v: v0 },
        c1: SpriteCorner {
            x: x + width,
            y,
            u: u1,
            v: v0,
        },
        c2: SpriteCorner {
            x: x + width,
            y: y + height,
            u: u1,
            v: v1,
        },
        c3: SpriteCorner {
            x,
            y: y + height,
            u: u0,
            v: v1,
        },
        color_rgba,
        source_over: true,
    }
}

fn report(message: std::fmt::Arguments<'_>) {
    logl::log(logl::level::ERROR, format_args!("hull: image {message}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_mock::{self, SERVICE};

    #[test]
    fn queued_assets_outlive_the_kernel_stage_deadlines() {
        let mut images = collect("<img src='queued.png'>");
        images.published();
        images.poll();
        images.assets[0].started = Some(Instant::now() - Duration::from_secs(80));
        assert!(!images.poll());
        assert!(!images.assets[0].finished);
        assert_eq!(images.assets[0].ticket, Some(1));
        images.assets[0].started = Some(Instant::now() - Duration::from_secs(91));
        assert!(!images.poll());
        assert!(images.assets[0].finished);
        assert!(images.assets[0].ticket.is_none());
    }

    fn collect(html: &str) -> Images {
        image_mock::reset();
        let document = rp5::parse(html);
        let mut css = crate::css::Css::default();
        css.parse_document(&document, "https://example.com/page/index.html");
        let layout = crate::layout::compute(&document, &css, (1280, 720)).unwrap();
        let mut images = Images::default();
        images.reset(1, 9);
        images.collect(
            &document,
            "https://example.com/page/index.html",
            layout.image_boxes,
        );
        images
    }

    #[test]
    fn first_base_https_canonicalization_query_dedupe_and_budget_preserve_all_boxes() {
        let mut html = String::from(
            "<base href='https://CDN.example:443/assets/'><base href='https://ignored.example/'><img src='./a.png?q=1#one'><img src='a.png?q=1#two'><img src='a.png?q=2'><img src='http://example.com/insecure.png'><img src='data:image/png;base64,no'>",
        );
        for index in 0..12 {
            html.push_str(&format!("<img src='{index}.png'>"));
        }
        let images = collect(&html);
        assert_eq!(images.boxes.len(), 17);
        assert_eq!(images.assets.len(), 8);
        assert_eq!(images.assets[0].url, "https://cdn.example/assets/a.png?q=1");
        assert_eq!(images.assets[1].url, "https://cdn.example/assets/a.png?q=2");
        assert_eq!(images.boxes[0].1, images.boxes[1].1);
        assert!(images.boxes[3].1.is_none());
        assert!(images.boxes[4].1.is_none());
        assert!(images.boxes[16].1.is_none());
        assert!(
            images
                .boxes
                .iter()
                .all(|(image, _)| image.rect[2..] == [300.0, 150.0])
        );
        SERVICE.with(|service| assert!(service.borrow().requests.is_empty()));
    }

    #[test]
    fn busy_reset_request_status_and_release_retry_without_losing_assets() {
        let mut images = collect("<img src='a.png'>");
        images.reset_pending = true;
        SERVICE.with(|service| service.borrow_mut().reset_busy = true);
        assert!(!images.poll());
        assert!(images.reset_pending);
        images.published();
        SERVICE.with(|service| service.borrow_mut().request_busy = true);
        assert!(!images.poll());
        assert!(!images.reset_pending);
        assert!(!images.assets[0].finished);
        assert!(images.assets[0].ticket.is_none());
        SERVICE.with(|service| service.borrow_mut().status_busy = true);
        assert!(!images.poll());
        assert_eq!(images.assets[0].ticket, Some(1));
        assert!(!images.assets[0].finished);
        SERVICE.with(|service| {
            let mut service = service.borrow_mut();
            service.live.get_mut(&1).unwrap().state = HULL_IMAGE_FAILED;
            service.release_busy = true;
        });
        images.poll();
        assert!(images.assets[0].finished);
        assert_eq!(images.assets[0].ticket, Some(1));
        images.poll();
        assert!(images.assets[0].ticket.is_none());
        SERVICE.with(|service| assert!(service.borrow().live.is_empty()));
    }

    #[test]
    fn admission_waits_for_publication_and_ready_completion_never_changes_geometry() {
        let mut images = collect(
            "<img src='a.png' width='160' height='90'><img src='a.png#duplicate' width='320' height='180'><img src='b.png'>",
        );
        let before: Vec<_> = images.boxes.iter().map(|(image, _)| image.rect).collect();
        for _ in 0..5 {
            assert!(!images.poll());
        }
        let mut viewport = Viewport::default();
        viewport.resize(1280, 720);
        let mut target = SceneTarget::default();
        assert!(!images.draw(&viewport, &mut target));
        assert_eq!(target.sprite_calls, 0);
        SERVICE.with(|service| assert!(service.borrow().requests.is_empty()));
        images.published();
        images.poll();
        SERVICE.with(|service| {
            let mut service = service.borrow_mut();
            assert_eq!(service.requests.len(), 1);
            assert_eq!(service.requests[0].1, "apps/hull");
            assert_eq!(service.requests[0].2, "example.com");
            assert_eq!((service.requests[0].3, service.requests[0].4), (320, 180));
            assert!(service.statuses <= 2);
            let status = service.live.get_mut(&1).unwrap();
            status.state = HULL_IMAGE_READY;
            status.sprite_id = 42;
            status.width = 1;
            status.height = 1;
        });
        assert!(images.poll());
        assert_eq!(images.assets[0].sprite, Some(42));
        assert_eq!(
            images
                .boxes
                .iter()
                .map(|(image, _)| image.rect)
                .collect::<Vec<_>>(),
            before
        );
        images.reset(1, 10);
        assert!(images.boxes.is_empty());
        SERVICE.with(|service| {
            let service = service.borrow();
            assert!(service.live.is_empty());
            assert_eq!(service.released.len(), 2);
        });
    }

    #[test]
    fn stale_or_failed_status_releases_handle_and_draw_error_keeps_placeholder() {
        let mut images = collect("<img src='a.png'>");
        images.published();
        images.poll();
        SERVICE.with(|service| {
            let mut service = service.borrow_mut();
            let info = service.live.get_mut(&1).unwrap();
            info.state = HULL_IMAGE_READY;
            info.sprite_id = 42;
        });
        assert!(images.poll());
        let mut viewport = Viewport::default();
        viewport.resize(100, 100);
        viewport.set_document(500, 500);
        viewport.scroll_by(75.0, 25.0);
        let mut target = SceneTarget::default();
        images.draw(&viewport, &mut target);
        assert_eq!(target.draws.len(), 1); // Ready alpha samples the page, not a placeholder underneath.
        let quad = target.draws[0];
        assert_eq!(quad.sprite_id, 42);
        assert!((quad.c0.u - 0.25).abs() < 1e-6 && (quad.c0.v - 1.0 / 6.0).abs() < 1e-6);
        target.draws.clear();
        target.fail_sprite = Some(42);
        assert!(images.draw(&viewport, &mut target));
        assert!(!images.draw(&viewport, &mut target));
        assert!(target.draws.is_empty());
        assert!(images.paint_disabled);
        assert!(images.assets[0].sprite.is_none());
        SERVICE.with(|service| assert!(service.borrow().live.is_empty()));

        let mut stale = collect("<img src='stale.png'>");
        stale.published();
        stale.poll();
        SERVICE.with(|service| service.borrow_mut().live.get_mut(&1).unwrap().generation = 8);
        assert!(!stale.poll());
        assert!(stale.assets[0].sprite.is_none());
        SERVICE.with(|service| assert!(service.borrow().live.is_empty()));
    }

    #[test]
    fn image_busy_disables_paint_and_recovers_retained_or_canceled_leases() {
        for cancel in [false, true] {
            let mut images = collect("<img src='a.png'><img src='pending.png'>");
            images.published();
            images.poll();
            images.poll();
            let mut viewport = Viewport::default();
            viewport.resize(800, 600);
            let mut target = SceneTarget {
                lease: true,
                cancel_on_sprite_error: cancel,
                draw_error: Some(Error::Busy),
                ..Default::default()
            };
            assert!(images.draw(&viewport, &mut target));
            assert!(images.paint_disabled);
            assert!(
                images
                    .assets
                    .iter()
                    .all(|asset| asset.finished && asset.sprite.is_none())
            );
            SERVICE.with(|service| assert!(service.borrow().live.is_empty()));
            assert_eq!(target.lease, !cancel);
            assert_eq!(recover_frame(&mut target), Ok(()));
            assert_eq!(target.text_probe(), Ok(()));
            assert!(!images.draw(&viewport, &mut target));
            assert_eq!(target.sprite_calls, 1);
        }
    }

    #[test]
    fn recovery_never_releases_quarantine_or_treats_unretired_text_as_ready() {
        let mut target = SceneTarget {
            quarantined: true,
            ..Default::default()
        };
        assert_eq!(recover_frame(&mut target), Err(Error::Busy));
        assert!(!target.lease);
        target.quarantined = false;
        target.lease = true;
        target.unretired = true;
        assert_eq!(recover_frame(&mut target), Ok(()));
        assert_eq!(target.text_probe(), Err(Error::InvalidState));
        assert!(target.lease);
        target.unretired = false;
        assert_eq!(target.text_probe(), Ok(()));
    }
}
