//! Document link actions: navigation opens a fresh Hull; download links are submitted to Trauma.
use super::{layout::LinkBox, shellio, viewport::Viewport};
use rp5::Document;
use std::collections::HashMap;
use url::Url;

#[derive(Default)]
pub(super) struct Links {
    boxes: Vec<LinkBox>,
    targets: HashMap<rp5::NodeId, LinkAction>,
    pressed: HashMap<Option<u64>, (LinkAction, f64, f64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LinkAction {
    Navigate(String),
    Download {
        url: String,
        filename_hint: Option<String>,
    },
}

/// File links use the shared type catalogue locally; no network preflight.
pub(super) fn download_url(text: &str) -> bool {
    let Ok(url) = Url::parse(text) else {
        return false;
    };
    let name = url.path().rsplit('/').next().unwrap_or("");
    let Some((_, extension)) = name.rsplit_once('.') else {
        return false;
    };
    infer::content_type_from_extension(extension)
        .and_then(infer::content_type_info)
        .is_some_and(|kind| !matches!(kind.mime_type, "text/html" | "application/xhtml+xml"))
}

impl Links {
    pub fn new(document: &Document, boxes: Vec<LinkBox>, page_url: &str) -> Self {
        let mut links = Self {
            boxes,
            ..Default::default()
        };
        let Ok(mut base) = Url::parse(page_url) else {
            return links;
        };
        for id in 0..document.nodes.len() {
            if document.name(id) == "base" {
                if let Some(href) = href(document, id) {
                    if let Ok(url) = base.join(href) {
                        base = url;
                    }
                    break;
                }
            }
        }
        for hit in &links.boxes {
            if links.targets.contains_key(&hit.node) {
                continue;
            }
            if let Some(url) = href(document, hit.node)
                .and_then(|href| base.join(href.trim()).ok())
                .and_then(|url| shellio::page_url(url.as_str()).ok())
            {
                let action = if has_download_attribute(document, hit.node) || download_url(&url) {
                    LinkAction::Download {
                        url,
                        filename_hint: download_hint(document, hit.node),
                    }
                } else {
                    LinkAction::Navigate(url)
                };
                links.targets.entry(hit.node).or_insert(action);
            }
        }
        links
    }

    /// Feedback follows painted text, not the anchor's enclosing block/images.
    pub fn hover_regions(&self, viewport: &Viewport) -> Vec<trueos::ui4_winit::ButtonRegion> {
        let fragments: Vec<_> = self
            .boxes
            .iter()
            .filter_map(|hit| {
                let color = hit.hover_color?;
                (color[3] != 0 && self.targets.contains_key(&hit.node)).then_some(
                    super::layout::ButtonBox {
                        rect: hit.rect,
                        border_color: color,
                    },
                )
            })
            .collect();
        super::buttons::regions(&fragments, viewport)
    }

    fn at(&self, x: f64, y: f64, viewport: &Viewport) -> Option<&LinkAction> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        self.boxes.iter().rev().find_map(|hit| {
            let [left, top, width, height] = viewport.clip_rect(hit.rect)?;
            (x >= f64::from(left)
                && x < f64::from(left + width)
                && y >= f64::from(top)
                && y < f64::from(top + height))
            .then(|| self.targets.get(&hit.node))
            .flatten()
        })
    }

    /// Activate only a press/release on the same link, with no drag. Native
    /// pointer endpoints have independent button state, including virtual mice.
    pub fn button(
        &mut self,
        pointer: Option<u64>,
        down: bool,
        x: f64,
        y: f64,
        viewport: &Viewport,
    ) -> Option<LinkAction> {
        if down {
            self.pressed.remove(&pointer);
            if let Some(action) = self.at(x, y, viewport).cloned() {
                self.pressed.insert(pointer, (action, x, y));
            }
            return None;
        }
        let (target, start_x, start_y) = self.pressed.remove(&pointer)?;
        if (x - start_x).hypot(y - start_y) <= 5.0 && self.at(x, y, viewport) == Some(&target) {
            Some(target)
        } else {
            None
        }
    }

    pub fn moved(&mut self, pointer: Option<u64>, x: f64, y: f64) {
        if self
            .pressed
            .get(&pointer)
            .is_some_and(|(_, start_x, start_y)| {
                !x.is_finite() || !y.is_finite() || (x - start_x).hypot(y - start_y) > 5.0
            })
        {
            self.pressed.remove(&pointer);
        }
    }

    pub fn cancel(&mut self) {
        self.pressed.clear();
    }
}

pub(super) fn href(document: &Document, id: rp5::NodeId) -> Option<&str> {
    let (_, _, attributes) = document.element(id)?;
    attributes
        .iter()
        .find(|attr| attr.namespace.is_none() && attr.name == "href")
        .map(|attr| attr.value.as_str())
}

fn has_download_attribute(document: &Document, id: rp5::NodeId) -> bool {
    document.element(id).is_some_and(|(_, _, attributes)| {
        attributes
            .iter()
            .any(|attr| attr.namespace.is_none() && attr.name == "download")
    })
}

fn download_hint(document: &Document, id: rp5::NodeId) -> Option<String> {
    document.element(id).and_then(|(_, _, attributes)| {
        attributes
            .iter()
            .find(|attr| attr.namespace.is_none() && attr.name == "download")
            .map(|attr| attr.value.clone())
            .filter(|value| !value.is_empty())
    })
}

/// Encode the endpoint request as URL bytes, a NUL separator, then an optional filename hint.
pub(super) fn download_request(action: &LinkAction) -> Result<Vec<u8>, &'static str> {
    let LinkAction::Download { url, filename_hint } = action else {
        return Err("link is not a download");
    };
    if url.len() > 4096 {
        return Err("download URL exceeds 4096 bytes");
    }
    if filename_hint.as_ref().is_some_and(|hint| hint.len() > 512) {
        return Err("download filename hint exceeds 512 bytes");
    }
    let mut bytes =
        Vec::with_capacity(url.len() + 1 + filename_hint.as_ref().map_or(0, String::len));
    bytes.extend_from_slice(url.as_bytes());
    bytes.push(0);
    if let Some(hint) = filename_hint {
        bytes.extend_from_slice(hint.as_bytes());
    }
    Ok(bytes)
}

pub(super) fn launch_script(target: &str) -> Result<String, &'static str> {
    Ok(format!("open {}\n", shellio::page_url(target)?))
}

/// The launch file is a one-shot control stream, separate from terminal input.
/// An empty launch is deliberately idle; no example document is fetched.
pub(super) fn startup_url(script: &str) -> Result<Option<String>, &'static str> {
    let mut target = None;
    for line in script
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let url = line
            .strip_prefix("open ")
            .ok_or("startup script expects: open <HTTPS URL>")?;
        if target.is_some() {
            return Err("startup script accepts one page URL");
        }
        target = Some(shellio::page_url(url)?);
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(href: &str) -> (Links, Viewport) {
        let document = rp5::parse(&format!(
            "<base href='https://assets.test/docs/'><a href='{href}'>link</a>"
        ));
        let node = (0..document.nodes.len())
            .find(|&id| document.name(id) == "a")
            .unwrap();
        let links = Links::new(
            &document,
            vec![LinkBox {
                node,
                rect: [20.0, 100.0, 80.0, 22.0],
                hover_color: None,
            }],
            "https://example.test/index",
        );
        let mut viewport = Viewport::default();
        viewport.resize_with_origin(200, 100, 0, 20);
        viewport.set_document(200, 400);
        viewport.scroll_by(0.0, 80.0);
        (links, viewport)
    }

    #[test]
    fn github_installer_click_goes_directly_to_trauma() {
        for url in [
            "https://github.com/ip7z/7zip/releases/download/26.05/7z2605.exe",
            "https://github.com/ip7z/7zip/releases/download/26.05/7z2605-x64.exe",
            "https://example.test/archive.zip?token=123",
            "https://example.test/image.PNG",
        ] {
            let (mut links, viewport) = fixture(url);
            links.button(Some(1), true, 30.0, 45.0, &viewport);
            assert!(
                matches!(
                    links.button(Some(1), false, 30.0, 45.0, &viewport),
                    Some(LinkAction::Download { .. })
                ),
                "{url}"
            );
        }
        for url in [
            "https://www.7-zip.org/",
            "https://example.test/index.html",
            "https://example.test/a.zip/page",
            "https://example.test/page?file=installer.exe",
        ] {
            assert!(!download_url(url), "{url}");
        }
    }

    #[test]
    fn click_resolves_base_relative_url_and_releases_only_once() {
        let (mut links, viewport) = fixture("../page?q=1&amp;x=2#part");
        assert!(links.button(Some(1), true, 30.0, 45.0, &viewport).is_none());
        let target = links.button(Some(1), false, 30.0, 45.0, &viewport).unwrap();
        assert_eq!(
            target,
            LinkAction::Navigate("https://assets.test/page?q=1&x=2".into())
        );
        let LinkAction::Navigate(url) = target else {
            panic!("ordinary links should navigate");
        };
        assert_eq!(
            startup_url(&launch_script(&url).unwrap()).unwrap(),
            Some(url)
        );
        assert!(
            links
                .button(Some(1), false, 30.0, 45.0, &viewport)
                .is_none()
        );
    }

    #[test]
    fn chrome_drag_focus_loss_and_other_pointers_do_not_activate() {
        let (mut links, viewport) = fixture("page");
        assert!(links.at(30.0, 10.0, &viewport).is_none());
        assert!(links.at(f64::NAN, 45.0, &viewport).is_none());
        links.button(Some(1), true, 30.0, 45.0, &viewport);
        assert!(
            links
                .button(Some(2), false, 30.0, 45.0, &viewport)
                .is_none()
        );
        links.moved(Some(1), 150.0, 45.0);
        assert!(
            links
                .button(Some(1), false, 30.0, 45.0, &viewport)
                .is_none()
        );
        links.button(Some(1), true, 30.0, 45.0, &viewport);
        links.cancel();
        assert!(
            links
                .button(Some(1), false, 30.0, 45.0, &viewport)
                .is_none()
        );
    }

    #[test]
    fn unsupported_link_schemes_never_activate() {
        for href in [
            "javascript:alert(1)",
            "mailto:person@example.test",
            "http://example.test/",
            "data:text/html,hi",
        ] {
            let (mut links, viewport) = fixture(href);
            links.button(None, true, 30.0, 45.0, &viewport);
            assert!(links.button(None, false, 30.0, 45.0, &viewport).is_none());
        }
    }

    #[test]
    fn explicit_download_links_submit_url_and_optional_filename_hint() {
        let document = rp5::parse(
            "<base href='https://assets.test/files/'><a href='report.csv' download='monthly.csv'>download</a>",
        );
        let node = (0..document.nodes.len())
            .find(|&id| document.name(id) == "a")
            .unwrap();
        let mut links = Links::new(
            &document,
            vec![LinkBox {
                node,
                rect: [20.0, 100.0, 80.0, 22.0],
                hover_color: None,
            }],
            "https://example.test/index",
        );
        let mut viewport = Viewport::default();
        viewport.resize_with_origin(200, 100, 0, 20);
        viewport.set_document(200, 400);
        viewport.scroll_by(0.0, 80.0);

        links.button(None, true, 30.0, 45.0, &viewport);
        let action = links.button(None, false, 30.0, 45.0, &viewport).unwrap();
        assert_eq!(
            action,
            LinkAction::Download {
                url: "https://assets.test/files/report.csv".into(),
                filename_hint: Some("monthly.csv".into()),
            }
        );
        assert_eq!(
            download_request(&action).unwrap(),
            b"https://assets.test/files/report.csv\0monthly.csv"
        );
    }

    #[test]
    fn empty_download_attribute_has_no_hint_and_request_limits_are_reported() {
        let document = rp5::parse("<a href='https://example.test/file' download>file</a>");
        let node = (0..document.nodes.len())
            .find(|&id| document.name(id) == "a")
            .unwrap();
        assert!(has_download_attribute(&document, node));
        assert_eq!(download_hint(&document, node), None);

        let action = LinkAction::Download {
            url: "https://example.test/file".into(),
            filename_hint: None,
        };
        assert_eq!(
            download_request(&action).unwrap(),
            b"https://example.test/file\0"
        );
        assert!(
            download_request(&LinkAction::Download {
                url: "x".repeat(4097),
                filename_hint: None,
            })
            .is_err()
        );
        assert!(
            download_request(&LinkAction::Download {
                url: "https://example.test/".into(),
                filename_hint: Some("x".repeat(513)),
            })
            .is_err()
        );
    }

    #[test]
    fn no_startup_url_is_idle_and_scripts_accept_exactly_one_navigation() {
        assert_eq!(startup_url(" \n").unwrap(), None);
        assert_eq!(
            startup_url("open https://example.test/page\n").unwrap(),
            Some("https://example.test/page".into())
        );
        for script in [
            "open https://a.test/\nopen https://b.test/",
            "run something",
            "open javascript:alert(1)",
        ] {
            assert!(startup_url(script).is_err());
        }
        assert!(launch_script("https://a.test/\nopen https://b.test/").is_err());
        assert!(launch_script(&format!("https://a.test/{}", "x".repeat(4096))).is_err());
    }
}

/// Last-line protection when GET disagrees with the navigation header probe.
pub(super) fn binary_download(bytes: &[u8]) -> bool {
    bytes.starts_with(b"MZ")
        || bytes.starts_with(b"\x7fELF")
        || bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"7z\xbc\xaf\x27\x1c")
        || bytes.starts_with(b"\x1f\x8b")
}

#[cfg(test)]
mod binary_tests {
    use super::binary_download;
    #[test]
    fn executables_and_archives_never_enter_the_html_parser() {
        for bytes in [
            &b"MZ\x90\0installer"[..],
            b"\x7fELFbinary",
            b"PK\x03\x04archive",
            b"7z\xbc\xaf\x27\x1carchive",
            b"\x1f\x8bcompressed",
        ] {
            assert!(binary_download(bytes));
        }
        assert!(!binary_download(
            b"<!doctype html><a href='installer.exe'>Download</a>"
        ));
        assert!(!binary_download(b""));
    }
}
