//! One-shot CSS parsing. HTML extraction stays in rp5; CSS syntax belongs to Lightning CSS.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use lightningcss::rules::CssRule;
use lightningcss::stylesheet::{ParserOptions, StyleAttribute, StyleSheet};
use static_self::IntoOwned;
use trueos::{logl, netfs};
use url::Url;
#[path = "style_index.rs"]
mod style_index;
pub(super) use style_index::{PreparedRule, PreparedRules, PreparedSelector, SelectorKey};

#[derive(Default)]
pub(super) struct Css {
    pub sheets: Vec<StyleSheet<'static>>,
    pub prepared_sheets: Vec<PreparedRules>,
    pub inline_by_node: HashMap<rp5::NodeId, usize>,
    pub attributes: Vec<(rp5::NodeId, StyleAttribute<'static>)>,
    downloads: HashMap<u32, Url>,
    requested: HashSet<String>,
    pub sources: Vec<CssSource>,
    pub roots: Vec<RootStylesheet>,
    pub external_sheets: HashMap<String, usize>,
    started: Option<Instant>,
    source_bytes: usize,
    frozen: bool,
}

const MAX_STYLESHEETS: usize = 16;
const MAX_SHEET_BYTES: usize = 512 * 1024;
const MAX_CSS_BYTES: usize = 2 * 1024 * 1024;
const CSS_WAIT: Duration = Duration::from_millis(2_000);

pub(super) struct RootStylesheet {
    pub source: SheetReference,
    pub media: Option<lightningcss::media_query::MediaList<'static>>,
}
pub(super) enum SheetReference {
    Embedded(usize),
    External(String),
}

pub(super) struct CssSource {
    pub node: Option<rp5::NodeId>,
    pub inline: bool,
    pub origin: String,
    pub code: String,
}

impl Css {
    pub fn parse_document(&mut self, document: &rp5::Document, page_url: &str) {
        self.started = Some(Instant::now());
        let Ok(mut base) = Url::parse(page_url) else {
            return;
        };
        let mut nodes = Vec::new();
        let mut pending = vec![document.root];
        while let Some(id) = pending.pop() {
            nodes.push(id);
            pending.extend(document.nodes[id].children.iter().rev().copied());
        }
        // HTML's first base[href] applies to all document-relative CSS URLs.
        for &id in &nodes {
            if let Some(("base", rp5::Namespace::Html, attrs)) = document.element(id) {
                if let Some(href) = attribute(attrs, "href") {
                    if let Ok(resolved) = base.join(href) {
                        base = resolved;
                    }
                    break;
                }
            }
        }
        for id in nodes {
            let Some((name, namespace, attrs)) = document.element(id) else {
                continue;
            };
            if let Some(code) = attribute(attrs, "style") {
                if !self.admit_source(code.len()) {
                    continue;
                }
                self.sources.push(CssSource {
                    node: Some(id),
                    inline: true,
                    origin: base.to_string(),
                    code: code.to_owned(),
                });
                match StyleAttribute::parse(code, ParserOptions::default()) {
                    Ok(style) => self.add_attribute(id, style.into_owned()),
                    Err(error) => report(format_args!("inline CSS node={id}: {error}")),
                }
            }
            if name == "style"
                && matches!(namespace, rp5::Namespace::Html | rp5::Namespace::Svg)
                && attribute(attrs, "type")
                    .is_none_or(|value| value.is_empty() || value.eq_ignore_ascii_case("text/css"))
            {
                let code = document.text_content(id);
                if !self.admit_source(code.len()) {
                    continue;
                }
                self.sources.push(CssSource {
                    node: Some(id),
                    inline: false,
                    origin: base.to_string(),
                    code: code.clone(),
                });
                if let Some(index) = self.parse_sheet(&code, &base) {
                    self.roots.push(RootStylesheet {
                        source: SheetReference::Embedded(index),
                        media: parse_media(attribute(attrs, "media")),
                    });
                }
            }
            if name == "link"
                && namespace == rp5::Namespace::Html
                && attribute(attrs, "rel").is_some_and(|rel| {
                    rel.split_ascii_whitespace()
                        .any(|token| token.eq_ignore_ascii_case("stylesheet"))
                })
            {
                if let Some(href) = attribute(attrs, "href") {
                    if let Some(url) = self.request(&base, href) {
                        self.roots.push(RootStylesheet {
                            source: SheetReference::External(url),
                            media: parse_media(attribute(attrs, "media")),
                        });
                    }
                }
            }
        }
        self.report_counts();
    }

    fn parse_sheet(&mut self, code: &str, source: &Url) -> Option<usize> {
        let options = ParserOptions {
            filename: source.to_string(),
            ..ParserOptions::default()
        };
        match StyleSheet::parse(code, options) {
            Ok(sheet) => {
                // Only stylesheet imports are downloaded. Images, fonts and scripts are untouched.
                for rule in &sheet.rules.0 {
                    if let CssRule::Import(import) = rule {
                        self.request(source, &import.url);
                    }
                }
                Some(self.add_sheet(sheet.into_owned()))
            }
            Err(error) => {
                report(format_args!("CSS {source}: {error}"));
                None
            }
        }
    }

    pub fn add_sheet(&mut self, sheet: StyleSheet<'static>) -> usize {
        let index = self.sheets.len();
        self.prepared_sheets.push(PreparedRules::new(
            &sheet.rules.0,
            sheet.sources.first().map(String::as_str).unwrap_or(""),
        ));
        self.sheets.push(sheet);
        index
    }

    pub fn add_attribute(&mut self, node: rp5::NodeId, style: StyleAttribute<'static>) {
        self.inline_by_node.insert(node, self.attributes.len());
        self.attributes.push((node, style));
    }

    fn request(&mut self, base: &Url, reference: &str) -> Option<String> {
        if self.frozen || reference.len() > 4096 {
            return None;
        }
        let mut url = match base.join(reference) {
            Ok(url) => url,
            Err(error) => {
                report(format_args!("CSS URL {reference:?}: {error}"));
                return None;
            }
        };
        url.set_fragment(None);
        if self.requested.contains(url.as_str()) {
            return Some(url.to_string());
        }
        if url.scheme() != "https" {
            report(format_args!(
                "CSS URL unsupported by HTTPS downloader: {url}"
            ));
            return None;
        }
        if self.requested.len() >= MAX_STYLESHEETS {
            return None;
        }
        self.requested.insert(url.to_string());
        match netfs::fetch_bytes(url.as_str().as_bytes()) {
            Ok(operation) => {
                self.downloads.insert(operation, url.clone());
            }
            Err(code) => report(format_args!("CSS download start {url}: {code}")),
        }
        Some(url.to_string())
    }

    pub fn poll_downloads(&mut self) {
        if self
            .started
            .is_some_and(|started| started.elapsed() >= CSS_WAIT)
        {
            self.freeze();
            return;
        }
        let completed: Vec<_> = self
            .downloads
            .keys()
            .copied()
            .filter(|&operation| !matches!(netfs::fetch_bytes_result_len(operation), Err(-8)))
            .take(4)
            .collect();
        for operation in completed {
            let source = self.downloads.remove(&operation).unwrap();
            let result = match netfs::fetch_bytes_result_len(operation) {
                Ok(len) if self.admit_source(len) => netfs::fetch_bytes_read(operation),
                Ok(_) => Err(-4),
                Err(code) => Err(code),
            };
            let _ = netfs::fetch_bytes_discard(operation);
            match result {
                Ok(bytes) => {
                    let code = String::from_utf8_lossy(&bytes).into_owned();
                    self.sources.push(CssSource {
                        node: None,
                        inline: false,
                        origin: source.to_string(),
                        code: code.clone(),
                    });
                    if let Some(index) = self.parse_sheet(&code, &source) {
                        self.external_sheets.insert(source.to_string(), index);
                    }
                }
                Err(code) => report(format_args!("CSS download {source}: {code}")),
            }
            self.report_counts();
        }
    }

    pub fn is_pending(&self) -> bool {
        !self.downloads.is_empty()
    }

    fn admit_source(&mut self, len: usize) -> bool {
        if len > MAX_SHEET_BYTES || len > MAX_CSS_BYTES.saturating_sub(self.source_bytes) {
            return false;
        }
        self.source_bytes += len;
        true
    }

    /// Layout uses the styles collected so far. Late CSS never moves an already
    /// published image box or delays text indefinitely.
    pub fn freeze(&mut self) {
        if !self.downloads.is_empty() {
            report(format_args!(
                "stylesheet deadline reached; using collected styles"
            ));
        }
        for (operation, _) in self.downloads.drain() {
            let _ = netfs::fetch_bytes_discard(operation);
        }
        self.frozen = true;
    }

    fn report_counts(&self) {
        logl::log(
            logl::level::INFO,
            format_args!(
                "hull: Lightning CSS parsed sheets={} inline={} pending={}",
                self.sheets.len(),
                self.attributes.len(),
                self.downloads.len()
            ),
        );
    }
}

impl Drop for Css {
    fn drop(&mut self) {
        for &operation in self.downloads.keys() {
            let _ = netfs::fetch_bytes_discard(operation);
        }
    }
}

fn attribute<'a>(attrs: &'a [rp5::Attribute], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|attr| attr.namespace.is_none() && attr.name == name)
        .map(|attr| attr.value.as_str())
}

fn report(message: std::fmt::Arguments<'_>) {
    logl::log(logl::level::ERROR, format_args!("hull: {message}"));
}

fn parse_media(value: Option<&str>) -> Option<lightningcss::media_query::MediaList<'static>> {
    let value = value.filter(|value| !value.trim().is_empty())?;
    let mut input = cssparser::ParserInput::new(value);
    let mut parser = cssparser::Parser::new(&mut input);
    match lightningcss::media_query::MediaList::parse(&mut parser, &ParserOptions::default()) {
        Ok(media) => Some(media.into_owned()),
        Err(error) => {
            report(format_args!("stylesheet media attribute: {error:?}"));
            Some(lightningcss::media_query::MediaList {
                media_queries: vec![lightningcss::media_query::MediaQuery {
                    qualifier: None,
                    media_type: lightningcss::media_query::MediaType::Print,
                    condition: None,
                }],
            })
        }
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn recursive_css_requests_are_bounded_and_deadline_freezes_collected_styles() {
        let mut html = String::from("<style>p{width:120px}</style>");
        for i in 0..100 {
            html.push_str(&format!("<link rel='stylesheet' href='{i}.css'>"));
        }
        let document = rp5::parse(&html);
        let mut css = Css::default();
        css.parse_document(&document, "https://example.com/");
        assert_eq!(css.downloads.len(), MAX_STYLESHEETS);
        assert_eq!(css.sheets.len(), 1);
        css.started = Some(Instant::now() - CSS_WAIT);
        css.poll_downloads();
        assert!(!css.is_pending());
        assert_eq!(css.sheets.len(), 1);
        assert!(
            css.request(&Url::parse("https://example.com/").unwrap(), "late.css")
                .is_none()
        );
    }

    #[test]
    fn stylesheet_byte_budget_refuses_oversized_sources_before_parsing() {
        let mut css = Css::default();
        assert!(!css.admit_source(MAX_SHEET_BYTES + 1));
        assert_eq!(css.source_bytes, 0);
        for _ in 0..MAX_CSS_BYTES / MAX_SHEET_BYTES {
            assert!(css.admit_source(MAX_SHEET_BYTES));
        }
        assert!(!css.admit_source(1));
    }
}
