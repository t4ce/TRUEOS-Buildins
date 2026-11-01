//! Default document navigation: every supported click opens a fresh Hull.
use super::{layout::LinkBox, shellio, viewport::Viewport};
use rp5::Document;
use std::collections::HashMap;
use url::Url;

#[derive(Default)]
pub(super) struct Links {
    boxes: Vec<LinkBox>,
    targets: HashMap<rp5::NodeId, String>,
    pressed: HashMap<Option<u64>, (String, f64, f64)>,
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
            if let Some(target) = href(document, hit.node)
                .and_then(|href| base.join(href.trim()).ok())
                .and_then(|url| shellio::page_url(url.as_str()).ok())
            {
                links.targets.entry(hit.node).or_insert(target);
            }
        }
        links
    }

    fn at(&self, x: f64, y: f64, viewport: &Viewport) -> Option<&str> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        self.boxes.iter().rev().find_map(|hit| {
            let [left, top, width, height] = viewport.clip_rect(hit.rect)?;
            (x >= f64::from(left)
                && x < f64::from(left + width)
                && y >= f64::from(top)
                && y < f64::from(top + height))
            .then(|| self.targets.get(&hit.node).map(String::as_str))
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
    ) -> Option<String> {
        if down {
            self.pressed.remove(&pointer);
            if let Some(target) = self.at(x, y, viewport).map(str::to_owned) {
                self.pressed.insert(pointer, (target, x, y));
            }
            return None;
        }
        let (target, start_x, start_y) = self.pressed.remove(&pointer)?;
        if (x - start_x).hypot(y - start_y) <= 5.0
            && self.at(x, y, viewport) == Some(target.as_str())
        {
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
    fn click_resolves_base_relative_url_and_releases_only_once() {
        let (mut links, viewport) = fixture("../page?q=1&amp;x=2#part");
        assert!(links.button(Some(1), true, 30.0, 45.0, &viewport).is_none());
        let target = links.button(Some(1), false, 30.0, 45.0, &viewport).unwrap();
        assert_eq!(target, "https://assets.test/page?q=1&x=2");
        assert_eq!(
            startup_url(&launch_script(&target).unwrap()).unwrap(),
            Some(target)
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
