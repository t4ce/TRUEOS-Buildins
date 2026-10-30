//! Extract script sources for inspection. Never parse, evaluate or execute them.
use std::collections::HashMap;

use rp5::{Document, Namespace, NodeId};
use trueos::logl;
use url::Url;

#[derive(Default)]
pub(super) struct Scripts {
    pub sources: Vec<ScriptSource>,
}

pub(super) struct ScriptSource {
    pub nodes: Vec<NodeId>,
    pub origin: String,
    pub kind: String,
    pub bytes: Option<Vec<u8>>,
}

impl Scripts {
    pub fn extract(&mut self, document: &Document, page_url: &str) {
        let Ok(mut base) = Url::parse(page_url) else {
            return;
        };
        let mut nodes = Vec::new();
        let mut pending = vec![document.root];
        while let Some(id) = pending.pop() {
            nodes.push(id);
            pending.extend(document.nodes[id].children.iter().rev().copied());
        }
        for &id in &nodes {
            if let Some(("base", Namespace::Html, attrs)) = document.element(id) {
                if let Some(href) = attribute(attrs, "href") {
                    if let Ok(url) = base.join(href) {
                        base = url;
                    }
                    break;
                }
            }
        }
        let mut external: HashMap<String, usize> = HashMap::new();
        for id in nodes {
            let Some(("script", namespace, attrs)) = document.element(id) else {
                continue;
            };
            if !matches!(namespace, Namespace::Html | Namespace::Svg) {
                continue;
            }
            let kind = attribute(attrs, "type")
                .unwrap_or("text/javascript")
                .to_owned();
            let src = attribute(attrs, "src").or_else(|| attribute(attrs, "href"));
            if let Some(src) = src {
                let mut url = match base.join(src) {
                    Ok(url) => url,
                    Err(error) => {
                        report(format_args!("script URL {src:?}: {error}"));
                        continue;
                    }
                };
                url.set_fragment(None);
                let origin = url.to_string();
                if let Some(&index) = external.get(&origin) {
                    self.sources[index].nodes.push(id);
                    continue;
                }
                let index = self.sources.len();
                external.insert(origin.clone(), index);
                self.sources.push(ScriptSource {
                    nodes: vec![id],
                    origin,
                    kind,
                    bytes: None,
                });
                // Keep the resolved source reference for inspection. Scripts
                // cannot affect this static renderer and consume no network work.
            } else {
                self.sources.push(ScriptSource {
                    nodes: vec![id],
                    origin: base.to_string(),
                    kind,
                    bytes: Some(document.text_content(id).into_bytes()),
                });
            }
        }
        self.report_counts();
    }

    fn report_counts(&self) {
        logl::log(
            logl::level::INFO,
            format_args!(
                "hull: scripts collected={} (inspection only; external sources are not fetched)",
                self.sources.len(),
            ),
        );
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
