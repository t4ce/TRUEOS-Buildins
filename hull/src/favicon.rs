//! Bounded favicon discovery. The image service remains the decoder and owner.
use alloc::{string::{String, ToString}, vec::Vec, format, vec};
use hashbrown::HashSet;

use rp5::{Document, Namespace};
use url::Url;

pub(super) const REQUEST_EDGE: u32 = 28;
const SLOT_EDGE: u32 = 32;
const MAX_CANDIDATES: usize = 16;

/// Prefer explicit icon links, then Apple touch icons, then the origin fallback.
/// SVG candidates use the native service's remote PNG bridge. Unknown types
/// are tried and may fail through to another candidate.
pub(super) fn discover(document: &Document, page_url: &str) -> Vec<String> {
    let Some(base) = document_base(document, page_url) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    let mut pending = vec![document.root];
    let mut order = 0usize;
    while let Some(id) = pending.pop() {
        if let Some(("link", Namespace::Html, attrs)) = document.element(id) {
            let rel = attribute(attrs, "rel").unwrap_or("");
            let icon = rel
                .split_ascii_whitespace()
                .any(|token| token.eq_ignore_ascii_case("icon"));
            let apple = rel.split_ascii_whitespace().any(|token| {
                token.eq_ignore_ascii_case("apple-touch-icon")
                    || token.eq_ignore_ascii_case("apple-touch-icon-precomposed")
            });
            if icon || apple {
                let mime = attribute(attrs, "type")
                    .unwrap_or("")
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim();
                if let Some(url) = attribute(attrs, "href").and_then(|href| canonical(&base, href))
                {
                    let path = Url::parse(&url)
                        .ok()
                        .map(|url| url.path().to_ascii_lowercase())
                        .unwrap_or_default();
                    let raster = ["image/png", "image/jpeg", "image/bmp"]
                        .iter()
                        .any(|kind| mime.eq_ignore_ascii_case(kind))
                        || [".png", ".jpg", ".jpeg", ".bmp"]
                            .iter()
                            .any(|extension| path.ends_with(extension));
                    let format_rank = if raster {
                        0
                    } else if path.ends_with(".ico")
                        || mime.eq_ignore_ascii_case("image/x-icon")
                        || mime.eq_ignore_ascii_case("image/vnd.microsoft.icon")
                    {
                        1
                    } else {
                        2
                    };
                    candidates.push((
                        (
                            u8::from(!icon),
                            size_score(attribute(attrs, "sizes")),
                            format_rank,
                            order,
                        ),
                        url,
                    ));
                }
            }
            order += 1;
        }
        pending.extend(document.nodes[id].children.iter().rev().copied());
    }
    candidates.sort_by_key(|(rank, _)| *rank);
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for (_, url) in candidates {
        if seen.insert(url.clone()) {
            urls.push(url);
            if urls.len() == MAX_CANDIDATES - 1 {
                break;
            }
        }
    }
    if let Some(url) = fallback(page_url) {
        if seen.insert(url.clone()) {
            urls.push(url);
        }
    }
    urls
}

pub(super) fn fallback(page_url: &str) -> Option<String> {
    let page = Url::parse(page_url).ok()?;
    canonical(&page, "/favicon.ico")
}

fn size_score(sizes: Option<&str>) -> u32 {
    sizes
        .unwrap_or("")
        .split_ascii_whitespace()
        .filter_map(|size| {
            if size.eq_ignore_ascii_case("any") {
                return Some(1);
            }
            let size = size.to_ascii_lowercase();
            let (width, height) = size.split_once('x')?;
            let (width, height) = (width.parse::<u32>().ok()?, height.parse::<u32>().ok()?);
            if width == 0 || height == 0 {
                return None;
            }
            Some(
                width
                    .abs_diff(SLOT_EDGE)
                    .saturating_add(height.abs_diff(SLOT_EDGE)),
            )
        })
        .min()
        .unwrap_or(1024)
}

fn canonical(base: &Url, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 4096 {
        return None;
    }
    let mut url = base.join(raw).ok()?;
    if !crate::resource_url::allowed(&url) {
        return None;
    }
    url.set_fragment(None);
    let value = url.to_string();
    (value.len() <= 4096).then_some(value)
}

fn document_base(document: &Document, page_url: &str) -> Option<Url> {
    let mut base = Url::parse(page_url).ok()?;
    let mut pending = vec![document.root];
    while let Some(id) = pending.pop() {
        if let Some(("base", Namespace::Html, attrs)) = document.element(id) {
            if let Some(href) = attribute(attrs, "href") {
                if let Ok(url) = base.join(href) {
                    base = url;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_use_first_base_rel_tokens_size_ranking_and_canonical_deduplication() {
        let document = rp5::parse(
            r#"
            <base href='https://CDN.example:443/assets/'>
            <base href='https://ignored.example/'>
            <link rel='SHORTCUT ICON' href='small.ico' sizes='16x16'>
            <link rel='ICON' href='./best.png?v=1#first' sizes='32X32'>
            <link rel='icon' href='best.png?v=1#duplicate' sizes='48x48'>
            <link rel='icon' href='vector.svg' type='IMAGE/SVG+XML' sizes='any'>
            <link rel='apple-touch-icon' href='//apple.example/touch.png' sizes='180x180'>
            <link rel='icon' href='http://insecure.example/icon.png'>
            <link rel='icon' href='https://user:secret@example.com/icon.png'>
            <link rel='manifest' href='manifest.json'>
        "#,
        );
        assert_eq!(
            discover(&document, "https://page.example/nested/index.html"),
            [
                "https://cdn.example/assets/best.png?v=1",
                "https://cdn.example/assets/vector.svg",
                "https://cdn.example/assets/small.ico",
                "https://apple.example/touch.png",
                "https://page.example/favicon.ico",
            ]
        );
    }

    #[test]
    fn fallback_is_origin_root_and_candidates_are_bounded_without_losing_fallback() {
        assert_eq!(
            fallback("https://EXAMPLE.com:443/nested/page?q=1#fragment").as_deref(),
            Some("https://example.com/favicon.ico")
        );
        assert!(fallback("http://example.com/").is_none());
        let mut html = String::new();
        for n in 0..40 {
            html.push_str(&format!("<link rel='icon' href='{n}.png' sizes='32x32'>"));
        }
        let urls = discover(&rp5::parse(&html), "https://example.com/page/");
        assert_eq!(urls.len(), MAX_CANDIDATES);
        assert_eq!(urls[0], "https://example.com/page/0.png");
        assert_eq!(urls.last().unwrap(), "https://example.com/favicon.ico");
    }

    #[test]
    fn svg_links_are_admitted_and_unsafe_links_fall_through_to_apple_then_root() {
        let document = rp5::parse(
            "<link rel=icon href='a.svg'><link rel=icon type='image/svg+xml' href='/vector'><link rel=icon href='data:image/png;base64,no'><link rel=APPLE-TOUCH-ICON-PRECOMPOSED href='/touch.png'>",
        );
        assert_eq!(
            discover(&document, "https://example.com/page"),
            [
                "https://example.com/a.svg",
                "https://example.com/vector",
                "https://example.com/touch.png",
                "https://example.com/favicon.ico"
            ]
        );
    }
}
