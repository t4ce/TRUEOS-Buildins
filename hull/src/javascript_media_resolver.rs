//! Bounded media-only subdocument inspection. No browsing contexts or rendering.
use alloc::{string::{String, ToString}, borrow::ToOwned, format, vec};
use super::{
    LocalPage,
    module_loader::{Loader, Prepared},
};
use crate::javascript_dom;
use hull_quickjs::Engine;
use rp5::{Document, Namespace};
use alloc::{collections::VecDeque, rc::Rc};
use core::cell::{Cell, RefCell};
use hashbrown::HashSet;
use crate::platform::{Duration, Instant};
#[derive(Default)]
pub(super) struct Resolver {
    initialized: bool,
    pending: VecDeque<(String, usize, String)>,
    seen: HashSet<String>,
}
fn attr<'a>(doc: &'a Document, id: usize, name: &str) -> Option<&'a str> {
    doc.element(id)?
        .2
        .iter()
        .find(|a| a.namespace.is_none() && a.name == name)
        .map(|a| a.value.as_str())
}
fn address(base: &str, text: &str) -> Option<String> {
    let u = url::Url::parse(base).ok()?.join(text).ok()?;
    (crate::resource_url::allowed(&u) && u.scheme() == "https").then(|| u.to_string())
}
// JSON-LD is data, not another executable script. Only an explicitly typed
// VideoObject's contentUrl is a media candidate; url/embedUrl/images are not.
fn metadata_source(value: &serde_json::Value, base: &str) -> Option<String> {
    let mut pending = vec![(value, 0usize)];
    let mut visited = 0;
    while let Some((value, depth)) = pending.pop() {
        visited += 1;
        if visited > 512 || depth > 8 {
            return None;
        }
        if let Some(items) = value.as_array() {
            if pending.len() + items.len() > 512 {
                return None;
            }
            pending.extend(items.iter().rev().map(|v| (v, depth + 1)));
            continue;
        }
        let Some(object) = value.as_object() else {
            continue;
        };
        let typed = object.get("@type").is_some_and(|t| {
            t.as_str() == Some("VideoObject")
                || t.as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("VideoObject")))
        });
        if typed {
            if let Some(url) = object
                .get("contentUrl")
                .and_then(|v| v.as_str())
                .and_then(|v| address(base, v))
            {
                return Some(url);
            }
        }
        if let Some(graph) = object.get("@graph") {
            pending.push((graph, depth + 1));
        }
    }
    None
}
impl Resolver {
    fn enqueue(&mut self, url: String, depth: usize, referrer: &str) {
        if depth <= 3 && self.seen.len() < 9 && self.seen.insert(url.clone()) {
            self.pending.push_back((url, depth, referrer.to_owned()));
        }
    }
    fn discover(&mut self, doc: &Document, base: &str, depth: usize, page: &LocalPage) -> bool {
        let mut pending = vec![doc.root];
        while let Some(id) = pending.pop() {
            pending.extend(doc.nodes[id].children.iter().rev().copied());
            let Some((name, Namespace::Html, _)) = doc.element(id) else {
                continue;
            };
            if name == "script"
                && attr(doc, id, "type").is_some_and(|t| {
                    t.split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .eq_ignore_ascii_case("application/ld+json")
                })
            {
                let text = doc.text_content(id);
                if text.len() <= 256 * 1024 {
                    if let Some(url) = serde_json::from_str::<serde_json::Value>(&text)
                        .ok()
                        .and_then(|v| metadata_source(&v, base))
                    {
                        page.media.borrow_mut().request(id, url, base);
                        return true;
                    }
                }
            }
            if name == "video" {
                let source = attr(doc, id, "src").or_else(|| {
                    doc.nodes[id].children.iter().find_map(|&n| {
                        (doc.name(n) == "source")
                            .then(|| attr(doc, n, "src"))
                            .flatten()
                    })
                });
                if let Some(url) = source.and_then(|s| address(base, s)) {
                    page.media.borrow_mut().request(id, url, base);
                    return true;
                }
            }
            if name == "iframe" {
                let label = format!(
                    "{} {} {} {}",
                    attr(doc, id, "title").unwrap_or(""),
                    attr(doc, id, "id").unwrap_or(""),
                    attr(doc, id, "name").unwrap_or(""),
                    attr(doc, id, "class").unwrap_or("")
                )
                .to_ascii_lowercase();
                if label.contains("video") || label.contains("player") {
                    if let Some(url) = attr(doc, id, "src").and_then(|s| address(base, s)) {
                        self.enqueue(url, depth + 1, base);
                    }
                }
            }
        }
        false
    }
    pub(super) fn poll(&mut self, loader: &mut Loader, parent: &mut Engine, page: &LocalPage) {
        if page.media.borrow().has_request() {
            self.pending.clear();
            self.initialized = true;
            return;
        }
        if !self.initialized {
            self.initialized = true;
            if let Some(doc) = &page.document {
                if self.discover(&doc.borrow(), &page.url, 0, page) {
                    self.pending.clear();
                    return;
                }
            }
        }
        let Some((url, depth, referrer)) = self.pending.front().cloned() else {
            return;
        };
        let bytes = match loader.source_with_referrer(&url, &referrer) {
            Prepared::Pending => return,
            Prepared::Failed(_) => {
                self.pending.pop_front();
                return;
            }
            Prepared::Ready(b) => b,
        };
        self.pending.pop_front();
        if bytes.is_empty() || bytes.len() > 256 * 1024 {
            loader.events.push(super::module_loader::Event {url,status:"failed",detail:if bytes.is_empty(){"media inspection: empty HTML response".into()}else{"media inspection: HTML exceeds 256 KiB".into()}});
            return;
        }
        let doc = rp5::parse(&String::from_utf8_lossy(&bytes));
        if self.discover(&doc, &url, depth, page) {
            self.pending.clear();
            return;
        }
        // Execute only this tiny page's inline control code in a disposable
        // realm. Location writes are captured, never applied. No external JS,
        // timers, frames, network exports, player UI, or new worker are created.
        let remaining = parent
            .execution_budget_remaining()
            .unwrap_or(Duration::ZERO);
        if remaining.is_zero() {
            return;
        }
        let start = Instant::now();
        let _ = self.inspect(
            doc,
            &url,
            depth,
            page,
            remaining.min(Duration::from_millis(25)),
        );
        parent.set_execution_budget(remaining.saturating_sub(start.elapsed()));
    }
    fn inspect(
        &mut self,
        doc: Document,
        url: &str,
        depth: usize,
        page: &LocalPage,
        budget: Duration,
    ) -> Result<(), String> {
        let mut scripts = crate::scripts::Scripts::default();
        scripts.extract_quiet(&doc, url);
        let click = doc.nodes.iter().enumerate().find_map(|(id, _)| {
            if doc.name(id) != "button" {
                return None;
            }
            let label = format!(
                "{} {}",
                attr(&doc, id, "id").unwrap_or(""),
                doc.text_content(id)
            )
            .to_ascii_lowercase();
            if label.contains("play") {
                attr(&doc, id, "onclick").map(str::to_owned)
            } else {
                None
            }
        });
        let mut engine = Engine::new()?;
        engine.set_execution_budget(budget);
        engine.install_console()?;
        let state: javascript_dom::SharedScriptState = Default::default();
        state.borrow_mut().media_navigation = true;
        javascript_dom::install_for_page_with_cookies(
            &mut engine,
            Rc::new(RefCell::new(doc)),
            page.window.clone(),
            Rc::new(Cell::new(None)),
            url,
            page.cookies.clone(),
            None,
            Some(page.media.clone()),
            state,
        )?;
        engine.eval(
            include_bytes!("javascript_player_kt.js"),
            "hull:media-player-adapter",
            false,
        )?;
        engine.eval(
            include_bytes!("javascript_player_mdcore.js"),
            "hull:media-source-adapter",
            false,
        )?;
        for source in scripts
            .sources
            .iter()
            .filter(|s| {
                ["", "text/javascript", "application/javascript"]
                    .contains(&s.kind.split(';').next().unwrap_or("").trim())
            })
            .filter_map(|s| s.bytes.as_ref())
            .filter(|b| b.len() <= 64 * 1024)
            .take(8)
        {
            if engine.execution_budget_exhausted() {
                break;
            }
            let _ = engine.eval(source, url, false);
        }
        if let Some(click) = click {
            let _ = engine.eval(click.as_bytes(), "hull:media-play-control", false);
        }
        let base = url::Url::parse(url).map_err(|e| e.to_string())?;
        let navigations = core::mem::take(&mut page.media.borrow_mut().inspection_navigation);
        for target in navigations {
            if let Some(target) = address(url, &target)
                .filter(|s| url::Url::parse(s).is_ok_and(|u| u.origin() == base.origin()))
            {
                self.enqueue(target, depth + 1, url);
            }
        }
        Ok(())
    }
}
