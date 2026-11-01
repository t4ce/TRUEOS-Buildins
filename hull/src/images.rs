//! Image references and frozen boxes only. Fetching, caching, decoding and GPU
//! upload belong to the kernel image service; Hull never receives image bytes.
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};
use trueos::logl;
use trueos::ui4_scene::{
    HULL_IMAGE_FAILED, HULL_IMAGE_READY, hull_image_release, hull_image_request, hull_image_reset,
    hull_image_status,
};
use trueos::ui4_winit::{Error, SceneTarget, SpriteCorner, SpriteQuad};
use url::Url;

use super::{favicon, layout::ImageBox, viewport::Viewport};

const MAX_ASSETS: usize = 42;
const POLLS_PER_TICK: usize = 2;
const MAX_VISIBLE_BOXES: usize = 2048;
// Only repeated admission failure is aged out here. Accepted tickets can wait
// behind 42 jobs on two fetch workers (or the kernel's one-worker fallback),
// which can legitimately exceed 90 seconds. Kernel stages and navigation own
// accepted-ticket deadlines and cancellation.
const IMAGE_ADMISSION_DEADLINE: Duration = Duration::from_secs(90);

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
    favicon_enabled: bool,
    favicon_disabled: bool,
    favicon_candidates: VecDeque<String>,
    favicon_asset: Option<usize>,
    favicon_dirty: bool,
}

struct Asset {
    url: String,
    size: (u32, u32),
    ticket: Option<u32>,
    sprite: Option<u32>,
    started: Option<Instant>,
    finished: bool,
    retired: bool,
    decoded_size: (u32, u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FaviconSprite {
    pub sprite_id: u32,
    pub width: u32,
    pub height: u32,
}

impl Asset {
    fn new(url: String, size: (u32, u32)) -> Self {
        Self {
            url,
            size,
            ticket: None,
            sprite: None,
            started: None,
            finished: false,
            retired: false,
            decoded_size: (0, 0),
        }
    }
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

    /// Call immediately after reset on successful navigation. One of the 42
    /// service slots is reserved for favicon work, sharing this generation.
    pub fn start_favicon(&mut self, page_url: &str) {
        self.favicon_enabled = true;
        self.favicon_disabled = false;
        self.configure_cache(page_url);
        self.set_favicon_candidates(favicon::fallback(page_url).into_iter().collect());
    }

    pub fn collect_favicon(&mut self, document: &rp5::Document, page_url: &str) {
        if self.favicon_disabled {
            return;
        }
        self.favicon_enabled = true;
        self.configure_cache(page_url);
        self.set_favicon_candidates(favicon::discover(document, page_url));
    }

    pub fn favicon(&self) -> Option<FaviconSprite> {
        if self.favicon_disabled {
            return None;
        }
        let asset = self.assets.get(self.favicon_asset?)?;
        let (width, height) = asset.decoded_size;
        if width == 0 || height == 0 {
            return None;
        }
        Some(FaviconSprite {
            sprite_id: asset.sprite?,
            width,
            height,
        })
    }

    /// A failed chrome sprite draw disables only favicon painting until reset.
    /// Page boxes that share the asset retain their existing ticket ownership.
    pub fn disable_favicon(&mut self) {
        self.favicon_disabled = true;
        self.favicon_candidates.clear();
        self.retire_favicon();
        self.favicon_dirty = true;
    }

    fn configure_cache(&mut self, page_url: &str) {
        self.root = super::snapshot::app_root();
        self.host = Url::parse(page_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
    }

    fn set_favicon_candidates(&mut self, urls: Vec<String>) {
        if let Some(index) = self.favicon_asset {
            if urls
                .first()
                .is_some_and(|url| *url == self.assets[index].url)
            {
                self.favicon_candidates = urls.into_iter().skip(1).collect();
                return;
            }
        }
        self.retire_favicon();
        self.favicon_candidates = urls.into();
        self.favicon_dirty = true;
    }

    fn retire_favicon(&mut self) {
        let Some(index) = self.favicon_asset.take() else {
            return;
        };
        if self.boxes.iter().any(|(_, asset)| *asset == Some(index)) {
            return;
        }
        let asset = &mut self.assets[index];
        asset.retired = true;
        asset.finished = true;
        asset.sprite = None;
        if let Some(window) = self.window {
            release_ticket(window, asset);
        }
    }

    fn choose_favicon(&mut self) {
        if !self.favicon_enabled || self.favicon_disabled {
            return;
        }
        if let Some(index) = self.favicon_asset {
            let asset = &self.assets[index];
            if !asset.finished || asset.sprite.is_some() || asset.ticket.is_some() {
                return;
            }
            self.favicon_asset = None;
        }
        while let Some(url) = self.favicon_candidates.pop_front() {
            let index = if let Some(index) = self
                .assets
                .iter()
                .position(|asset| !asset.retired && asset.url == url)
            {
                index
            } else {
                let index = self.assets.len();
                self.assets.push(Asset::new(
                    url,
                    (favicon::REQUEST_EDGE, favicon::REQUEST_EDGE),
                ));
                index
            };
            let asset = &self.assets[index];
            if asset.finished && asset.sprite.is_none() && asset.ticket.is_none() {
                continue;
            }
            self.favicon_asset = Some(index);
            self.favicon_dirty = true;
            break;
        }
    }

    /// Collect all paintable boxes, including those with no source or over the
    /// admission limit. Such boxes retain a placeholder and their flow space.
    pub fn collect(&mut self, document: &rp5::Document, page_url: &str, boxes: Vec<ImageBox>) {
        self.configure_cache(page_url);
        let base = document_base(document, page_url);
        let mut admitted: HashMap<String, usize> = self
            .boxes
            .iter()
            .filter_map(|(_, index)| index.map(|index| (self.assets[index].url.clone(), index)))
            .collect();
        let limit = MAX_ASSETS - usize::from(self.favicon_enabled);
        for image in boxes {
            let url = base.as_ref().and_then(|base| {
                let (_, rp5::Namespace::Html, attrs) = document.element(image.node)? else {
                    return None;
                };
                image_source(attrs, base)
            });
            let size = request_size(image.rect);
            let asset = url.and_then(|url| {
                if let Some(&index) = admitted.get(&url) {
                    return Some(index);
                }
                let size = size?;
                if admitted.len() >= limit {
                    return None;
                }
                let index = if let Some(index) = self
                    .assets
                    .iter()
                    .position(|asset| !asset.retired && asset.url == url)
                {
                    index
                } else {
                    let index = self.assets.len();
                    self.assets.push(Asset::new(url.clone(), size));
                    index
                };
                admitted.insert(url, index);
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
        self.choose_favicon();
        let mut changed = std::mem::take(&mut self.favicon_dirty);
        // Chrome discovery stays inert until this navigation's content has
        // published; declared links and shared page target sizes settle first.
        if !self.published {
            return changed;
        }
        let favicon_request = self
            .favicon_asset
            .filter(|&index| !self.assets[index].finished && self.assets[index].ticket.is_none());
        let request = favicon_request.or_else(|| {
            self.published
                .then(|| {
                    self.assets.iter().position(|asset| {
                        !asset.retired && !asset.finished && asset.ticket.is_none()
                    })
                })
                .flatten()
        });
        // A retired variant can still be awaiting a Busy release. Do not let
        // kernel deduplication return that same ticket to a new asset owner.
        let request = request.filter(|&index| {
            !self.assets.iter().enumerate().any(|(other, asset)| {
                other != index && asset.ticket.is_some() && asset.url == self.assets[index].url
            })
        });
        // At most one admission and two status reads across page and icon work.
        if self
            .assets
            .iter()
            .filter(|asset| asset.ticket.is_some())
            .count()
            < MAX_ASSETS
        {
            if let Some(index) = request {
                let asset = &mut self.assets[index];
                let started = asset.started.get_or_insert_with(Instant::now);
                if started.elapsed() > IMAGE_ADMISSION_DEADLINE {
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
                            if Some(index) != self.favicon_asset {
                                report(format_args!(
                                    "request failed: {error:?} source={}",
                                    diagnostic_source(&asset.url)
                                ));
                            }
                        }
                    }
                }
            }
        }
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
            match hull_image_status(window, ticket) {
                Ok(info) if info.generation != self.generation => asset.finished = true,
                Ok(info)
                    if info.state == HULL_IMAGE_READY
                        && info.sprite_id != 0
                        && info.width != 0
                        && info.height != 0 =>
                {
                    asset.sprite = Some(info.sprite_id);
                    asset.decoded_size = (info.width, info.height);
                    asset.finished = true;
                    changed = true;
                }
                Ok(info) if info.state == HULL_IMAGE_READY => asset.finished = true,
                Ok(info) if info.state == HULL_IMAGE_FAILED => {
                    asset.finished = true;
                    if Some(index) != self.favicon_asset {
                        report(format_args!(
                            "asset failed: {} source={}",
                            info.error,
                            diagnostic_source(&asset.url)
                        ));
                    }
                }
                Ok(_) | Err(Error::Busy) => {}
                Err(error) => {
                    asset.finished = true;
                    if Some(index) != self.favicon_asset {
                        report(format_args!(
                            "status failed: {error:?} source={}",
                            diagnostic_source(&asset.url)
                        ));
                    }
                }
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
                let page_assets: HashSet<_> =
                    self.boxes.iter().filter_map(|(_, index)| *index).collect();
                for (index, asset) in self.assets.iter_mut().enumerate() {
                    if page_assets.contains(&index) && Some(index) != self.favicon_asset {
                        asset.finished = true;
                        asset.sprite = None;
                        release_ticket(target.render_target(), asset);
                    }
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

/// Static lazy-image attributes carry the intended source while `src` may be
/// empty or a transparent loading placeholder. Prefer modern data-src, then
/// legacy data-original, then ordinary src. Validate each independently so an
/// unsafe/invalid lazy value cannot hide a usable ordinary HTTPS source. This
/// selects a reference only: no script runs and the frozen box is untouched.
fn image_source(attrs: &[rp5::Attribute], base: &Url) -> Option<String> {
    for name in ["data-src", "data-original", "src"] {
        let Some(raw) = attribute(attrs, name) else {
            continue;
        };
        let raw = raw.trim();
        if raw.is_empty() || raw.len() > 4096 {
            continue;
        }
        let Ok(mut url) = base.join(raw) else {
            continue;
        };
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            continue;
        }
        url.set_fragment(None);
        return Some(url.to_string());
    }
    None
}

/// Identify the failed resource without logging URL credentials or signed
/// query/fragment data. Bound the public host/path so one failed asset cannot
/// produce a multi-kilobyte log line.
fn diagnostic_source(raw: &str) -> String {
    let Ok(url) = Url::parse(raw) else {
        return "<invalid-image-url>".into();
    };
    let Some(host) = url.host_str() else {
        return "<invalid-image-url>".into();
    };
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let full = format!("{authority}{}", url.path());
    let mut short: String = full.chars().take(256).collect();
    if short.len() < full.len() {
        short.push('…');
    }
    short
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

    fn collect_icons(html: &str) -> Images {
        image_mock::reset();
        let document = rp5::parse(html);
        let mut css = crate::css::Css::default();
        let url = "https://example.com/page/index.html";
        css.parse_document(&document, url);
        let layout = crate::layout::compute(&document, &css, (1280, 720)).unwrap();
        let mut images = Images::default();
        images.reset(1, 9);
        images.start_favicon(url);
        images.collect_favicon(&document, url);
        images.collect(&document, url, layout.image_boxes);
        images
    }

    #[test]
    fn favicon_reserves_one_shared_ticket_and_page_images_still_wait_for_publication() {
        let mut html = String::from("<link rel='icon' href='/icon.png' sizes='32x32'>");
        for n in 0..43 {
            html.push_str(&format!("<img src='{n}.png'>"));
        }
        let mut images = collect_icons(&html);
        assert_eq!(images.boxes.len(), 43);
        assert_eq!(
            images
                .boxes
                .iter()
                .filter(|(_, asset)| asset.is_some())
                .count(),
            41
        );
        for _ in 0..4 {
            images.poll();
        }
        SERVICE.with(|service| assert!(service.borrow().requests.is_empty()));
        images.published();
        images.poll();
        SERVICE.with(|service| {
            let service = service.borrow();
            assert_eq!(service.requests.len(), 1);
            assert_eq!(service.requests[0].0, "https://example.com/icon.png");
            assert_eq!((service.requests[0].3, service.requests[0].4), (28, 28));
            assert_eq!(service.requests[0].5, 9);
            assert!(service.statuses <= 2);
        });
        assert!(images.boxes[40].1.is_some());
        assert!(images.boxes[41].1.is_none());
        for _ in 0..45 {
            let (requests, statuses) = SERVICE.with(|service| {
                let service = service.borrow();
                (service.requests.len(), service.statuses)
            });
            images.poll();
            SERVICE.with(|service| {
                let service = service.borrow();
                assert!(service.requests.len() - requests <= 1);
                assert!(service.statuses - statuses <= 2);
            });
        }
        SERVICE.with(|service| {
            let service = service.borrow();
            assert_eq!(service.requests.len(), 42);
            assert_eq!(service.live.len(), 42);
        });
    }

    #[test]
    fn failed_favicon_candidates_release_before_retry_and_ready_extent_preserves_aspect() {
        let mut images = collect_icons(
            "<link rel=icon href='/bad.png' sizes='32x32'><link rel=icon href='/good.png' sizes='16x16'>",
        );
        images.published();
        images.poll();
        SERVICE.with(|service| {
            let mut service = service.borrow_mut();
            let info = service.live.get_mut(&1).unwrap();
            info.state = HULL_IMAGE_FAILED;
            info.error = -8;
            service.release_busy = true;
        });
        images.poll();
        for _ in 0..8 {
            images.poll();
        }
        SERVICE.with(|service| {
            let mut service = service.borrow_mut();
            assert_eq!(service.requests.len(), 2);
            assert_eq!(service.requests[1].0, "https://example.com/good.png");
            assert_eq!(service.live.len(), 1);
            assert!(service.released.contains(&1));
            let info = service.live.get_mut(&2).unwrap();
            info.state = HULL_IMAGE_READY;
            info.sprite_id = 99;
            info.width = 28;
            info.height = 14;
        });
        assert!(images.poll());
        assert_eq!(
            images.favicon(),
            Some(FaviconSprite {
                sprite_id: 99,
                width: 28,
                height: 14
            })
        );
        images.reset(1, 10);
        assert!(images.favicon().is_none());
        SERVICE.with(|service| assert!(service.borrow().live.is_empty()));
    }

    #[test]
    fn declared_icon_replaces_the_unadmitted_root_fallback_before_publication() {
        image_mock::reset();
        let mut images = Images::default();
        images.reset(1, 9);
        images.start_favicon("https://example.com/page");
        images.poll();
        let document = rp5::parse("<link rel=icon href='/preferred.png'>");
        images.collect_favicon(&document, "https://example.com/page");
        images.poll();
        SERVICE.with(|service| assert!(service.borrow().requests.is_empty()));
        images.published();
        images.poll();
        SERVICE.with(|service| {
            let service = service.borrow();
            assert_eq!(service.generation, 9);
            assert_eq!(service.requests.len(), 1);
            assert_eq!(service.requests[0].0, "https://example.com/preferred.png");
            assert!(service.released.is_empty());
            assert_eq!(service.live.len(), 1);
        });
    }

    #[test]
    fn disabling_favicon_keeps_a_shared_page_image_and_does_not_retry_until_navigation() {
        let html = "<link rel=icon href='/shared.png'><img src='/shared.png' width=28 height=28><img src='/other.png'>";
        let mut images = collect_icons(html);
        images.published();
        images.poll();
        images.poll();
        SERVICE.with(|service| {
            let mut service = service.borrow_mut();
            assert_eq!(service.requests.len(), 2);
            assert_eq!(
                service
                    .requests
                    .iter()
                    .filter(|request| request.0.ends_with("shared.png"))
                    .count(),
                1
            );
            let info = service.live.get_mut(&1).unwrap();
            info.state = HULL_IMAGE_READY;
            info.sprite_id = 42;
        });
        images.poll();
        assert!(images.favicon().is_some());
        images.disable_favicon();
        assert!(images.favicon().is_none());
        assert_eq!(images.assets[images.boxes[0].1.unwrap()].sprite, Some(42));
        images.collect_favicon(&rp5::parse(html), "https://example.com/page");
        for _ in 0..5 {
            images.poll();
        }
        SERVICE.with(|service| {
            let service = service.borrow();
            assert_eq!(service.requests.len(), 2);
            assert!(service.live.contains_key(&1));
        });
        images.reset(1, 10);
        SERVICE.with(|service| assert!(service.borrow().live.is_empty()));
    }

    #[test]
    fn exhausted_declared_candidates_try_origin_fallback_once_and_leave_no_live_tickets() {
        let mut images = collect_icons(
            "<link rel=icon href='/bad.png'><link rel=apple-touch-icon href='/touch.png'>",
        );
        images.published();
        for (index, expected) in [
            "https://example.com/bad.png",
            "https://example.com/touch.png",
            "https://example.com/favicon.ico",
        ]
        .into_iter()
        .enumerate()
        {
            for _ in 0..12 {
                images.poll();
                if SERVICE.with(|service| service.borrow().requests.len()) > index {
                    break;
                }
            }
            SERVICE.with(|service| {
                let mut service = service.borrow_mut();
                assert_eq!(service.requests[index].0, expected);
                let info = service.live.get_mut(&(index as u32 + 1)).unwrap();
                info.state = HULL_IMAGE_FAILED;
                info.error = -8;
            });
            for _ in 0..4 {
                images.poll();
            }
        }
        for _ in 0..12 {
            images.poll();
        }
        assert!(images.favicon().is_none());
        SERVICE.with(|service| {
            let service = service.borrow();
            assert_eq!(service.requests.len(), 3);
            assert!(service.live.is_empty());
        });
    }

    #[test]
    fn disabling_a_private_icon_retries_busy_release_but_never_admits_another_candidate() {
        let mut images = collect_icons("<link rel=icon href='/private.png'>");
        images.published();
        images.poll();
        SERVICE.with(|service| {
            let mut service = service.borrow_mut();
            let info = service.live.get_mut(&1).unwrap();
            info.state = HULL_IMAGE_READY;
            info.sprite_id = 77;
        });
        images.poll();
        assert!(images.favicon().is_some());
        SERVICE.with(|service| service.borrow_mut().release_busy = true);
        images.disable_favicon();
        assert!(images.favicon().is_none());
        for _ in 0..8 {
            images.poll();
        }
        SERVICE.with(|service| {
            let service = service.borrow();
            assert_eq!(service.requests.len(), 1);
            assert!(service.live.is_empty());
        });
        images.reset(1, 10);
        images.start_favicon("https://new.example/path");
        images.published();
        images.poll();
        SERVICE.with(|service| {
            let service = service.borrow();
            assert_eq!(service.requests.len(), 2);
            assert_eq!(service.requests[1].0, "https://new.example/favicon.ico");
            assert_eq!(service.requests[1].5, 10);
        });
    }

    #[test]
    fn static_lazy_sources_replace_empty_or_gif_placeholders_without_mutating_layout_or_svg() {
        image_mock::reset();
        let placeholder =
            "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
        let html = format!(
            "<base href='https://CDN.example:443/assets/'><base href='https://ignored.example/'><img data-src='./emoji.png?q=1#fragment' width=24 height=24><img src='{placeholder}' class='lazy-load' data-original='../thumb.jpg?q=2#ignored' width=336 height=189>"
        );
        let document = rp5::parse(&html);
        let mut css = crate::css::Css::default();
        let page = "https://example.com/page/index.html";
        css.parse_document(&document, page);
        let layout = crate::layout::compute(&document, &css, (1280, 720)).unwrap();
        let before_svg = layout.svg.clone();
        let before_rects: Vec<_> = layout.image_boxes.iter().map(|image| image.rect).collect();
        let mut images = Images::default();
        images.reset(1, 9);
        images.collect(&document, page, layout.image_boxes);
        assert_eq!(images.assets.len(), 2);
        assert_eq!(
            images.assets[0].url,
            "https://cdn.example/assets/emoji.png?q=1"
        );
        assert_eq!(images.assets[1].url, "https://cdn.example/thumb.jpg?q=2");
        let (_, _, attrs) = document.element(images.boxes[1].0.node).unwrap();
        assert_eq!(attribute(attrs, "src"), Some(placeholder));
        images.published();
        images.poll();
        images.poll();
        SERVICE.with(|service| {
            let mut service = service.borrow_mut();
            assert_eq!(service.requests.len(), 2);
            for (id, info) in &mut service.live {
                info.state = HULL_IMAGE_READY;
                info.sprite_id = 100 + *id;
                info.width = 1;
                info.height = 1;
            }
        });
        images.poll();
        assert_eq!(
            images
                .boxes
                .iter()
                .map(|(image, _)| image.rect)
                .collect::<Vec<_>>(),
            before_rects
        );
        assert_eq!(
            crate::layout::compute(&document, &css, (1280, 720))
                .unwrap()
                .svg,
            before_svg
        );
    }

    #[test]
    fn lazy_source_precedence_validates_each_candidate_and_falls_back_to_secure_src() {
        let images = collect(
            "<base href='https://cdn.example/assets/'><img src='normal.jpg' data-src='modern.jpg' data-original='legacy.jpg'><img src='fallback.jpg?q=1#fragment' data-src='http://insecure.example/a.jpg' data-original='https://user:password@example.com/a.jpg'><img src='ordinary.jpg' data-src=' ' data-original='//other.example/legacy.jpg#ignored'><img src='data:image/gif;base64,placeholder' data-src='javascript:alert(1)' data-original='http://insecure.example/legacy.jpg'>",
        );
        assert_eq!(images.boxes.len(), 4);
        assert_eq!(images.assets.len(), 3);
        assert_eq!(
            images.assets[0].url,
            "https://cdn.example/assets/modern.jpg"
        );
        assert_eq!(
            images.assets[1].url,
            "https://cdn.example/assets/fallback.jpg?q=1"
        );
        assert_eq!(images.assets[2].url, "https://other.example/legacy.jpg");
        assert!(images.boxes[3].1.is_none());
        SERVICE.with(|service| assert!(service.borrow().requests.is_empty()));
    }

    #[test]
    fn lazy_images_keep_canonical_deduplication_and_the_favicon_reservation() {
        let mut html = String::from(
            "<img data-src='same.jpg?x=1#one'><img src='data:image/gif;base64,placeholder' data-original='same.jpg?x=1#two'>",
        );
        for index in 0..42 {
            html.push_str(&format!("<img data-src='{index}.png'>"));
        }
        let images = collect_icons(&html);
        assert_eq!(images.boxes.len(), 44);
        assert_eq!(images.boxes[0].1, images.boxes[1].1);
        assert_eq!(
            images
                .boxes
                .iter()
                .filter_map(|(_, asset)| *asset)
                .collect::<HashSet<_>>()
                .len(),
            41
        );
        assert!(images.boxes[41].1.is_some());
        assert!(images.boxes[42].1.is_none());
        assert!(images.boxes.last().unwrap().1.is_none());
        SERVICE.with(|service| assert!(service.borrow().requests.is_empty()));
    }

    #[test]
    fn failure_source_diagnostics_strip_url_secrets_and_bound_public_path_length() {
        assert_eq!(
            diagnostic_source(
                "https://user:password@EXAMPLE.com:8443/public/photo.jpg?token=secret#fragment"
            ),
            "example.com:8443/public/photo.jpg"
        );
        assert_eq!(diagnostic_source("not a URL"), "<invalid-image-url>");
        let long = diagnostic_source(&format!(
            "https://example.com/{}?token=secret",
            "a".repeat(4000)
        ));
        assert!(long.chars().count() <= 257);
        assert!(!long.contains("secret"));
    }

    #[test]
    fn admitted_pending_tickets_outlive_queue_delays_until_kernel_terminal_status() {
        let mut images = collect("<img src='queued.png'>");
        images.published();
        images.poll();
        for age in [316, 631, 3600] {
            images.assets[0].started = Some(Instant::now() - Duration::from_secs(age));
            assert!(!images.poll());
            assert!(!images.assets[0].finished);
            assert_eq!(images.assets[0].ticket, Some(1));
        }
        SERVICE.with(|service| {
            let mut service = service.borrow_mut();
            let info = service.live.get_mut(&1).unwrap();
            info.state = HULL_IMAGE_FAILED;
            info.error = -6;
        });
        images.poll();
        assert!(images.assets[0].finished);
        assert!(images.assets[0].ticket.is_none());
        SERVICE.with(|service| assert!(service.borrow().live.is_empty()));
    }

    #[test]
    fn repeated_busy_admission_still_expires_after_ninety_seconds_without_a_ticket() {
        let mut images = collect("<img src='not-admitted.png'>");
        images.published();
        SERVICE.with(|service| service.borrow_mut().request_busy = true);
        images.poll();
        assert!(images.assets[0].started.is_some());
        assert!(!images.assets[0].finished);
        assert!(images.assets[0].ticket.is_none());
        SERVICE.with(|service| assert!(!service.borrow().request_busy));
        images.assets[0].started = Some(Instant::now() - Duration::from_secs(91));
        SERVICE.with(|service| service.borrow_mut().request_busy = true);
        images.poll();
        assert!(images.assets[0].finished);
        assert!(images.assets[0].ticket.is_none());
        SERVICE.with(|service| {
            let service = service.borrow();
            assert!(service.request_busy); // No new admission attempted after expiry.
            assert!(service.requests.is_empty());
            assert!(service.live.is_empty());
        });
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
        for index in 0..42 {
            html.push_str(&format!("<img src='{index}.png'>"));
        }
        let images = collect(&html);
        assert_eq!(images.boxes.len(), 47);
        assert_eq!(images.assets.len(), 42);
        assert_eq!(images.assets[0].url, "https://cdn.example/assets/a.png?q=1");
        assert_eq!(images.assets[1].url, "https://cdn.example/assets/a.png?q=2");
        assert_eq!(images.boxes[0].1, images.boxes[1].1);
        assert!(images.boxes[3].1.is_none());
        assert!(images.boxes[4].1.is_none());
        assert_eq!(images.assets[41].url, "https://cdn.example/assets/39.png");
        assert!(images.boxes[44].1.is_some());
        assert!(images.boxes[45].1.is_none());
        assert!(images.boxes[46].1.is_none());
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
