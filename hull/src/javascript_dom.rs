//! A synchronous, bounded gateway to rp5. JavaScript owns lazy node wrappers;
//! the document arena stays in Rust and preserves handles after detachment.
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use hull_quickjs::Engine;
use rp5::{Attribute, Document, Namespace, NodeKind};
use serde_json::{Value, json};

#[path = "javascript_jquery.rs"]
mod jquery;

pub(super) type SharedDocument = Rc<RefCell<Document>>;
pub(super) type SharedCurrentScript = Rc<Cell<Option<usize>>>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WindowMetrics {
    pub inner_width: u32,
    pub inner_height: u32,
}
pub(super) type SharedWindowMetrics = Rc<Cell<WindowMetrics>>;
const MAX_STRING_BYTES: usize = 256 * 1024;
const MAX_ADDED_NODES: usize = 4096;
const MAX_MUTATION_BYTES: usize = 4 * 1024 * 1024;
const MAX_QUERY_RESULTS: usize = 32 * 1024;

pub(super) fn install(engine: &mut Engine, document: SharedDocument) -> Result<(), String> {
    install_gateway(engine, document, None, Rc::new(Cell::new(None)))
}

/// Window metrics come from Hull's content viewport, not native frame guesses.
pub(super) fn install_with_window(
    engine: &mut Engine,
    document: SharedDocument,
    window: SharedWindowMetrics,
) -> Result<(), String> {
    install_gateway(engine, document, Some(window), Rc::new(Cell::new(None)))
}

/// The actor owns script execution scope; the gateway only reads its handle.
pub(super) fn install_for_page(
    engine: &mut Engine,
    document: SharedDocument,
    window: Option<SharedWindowMetrics>,
    current_script: SharedCurrentScript,
) -> Result<(), String> {
    install_gateway(engine, document, window, current_script)
}

fn install_gateway(
    engine: &mut Engine,
    document: SharedDocument,
    window: Option<SharedWindowMetrics>,
    current_script: SharedCurrentScript,
) -> Result<(), String> {
    let initial_nodes = document.borrow().nodes.len();
    let mut gateway = Gateway {
        document,
        window,
        current_script,
        initial_nodes,
        mutation_bytes: 0,
    };
    engine.bind_host_handler("__hullDom", move |bytes| {
        let request: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        let response = match gateway.call(&request) {
            Ok(value) => json!({"ok":true,"value":value}),
            Err((name, message)) => json!({"ok":false,"name":name,"message":message}),
        };
        serde_json::to_vec(&response).map_err(|error| error.to_string())
    })?;
    let source = include_str!("javascript_dom.js").replace(
        "/* HULL_NATIVE_JQUERY */",
        include_str!("javascript_jquery.js"),
    );
    engine.eval(source.as_bytes(), "hull:dom", false)
}

type DomResult<T> = Result<T, (&'static str, String)>;
fn failure(name: &'static str, message: impl Into<String>) -> (&'static str, String) {
    (name, message.into())
}
fn input<'a>(request: &'a [Value], index: usize) -> DomResult<&'a str> {
    request
        .get(index)
        .and_then(Value::as_str)
        .ok_or_else(|| failure("TypeError", "Expected a DOM string"))
}

struct Gateway {
    document: SharedDocument,
    window: Option<SharedWindowMetrics>,
    current_script: SharedCurrentScript,
    initial_nodes: usize,
    mutation_bytes: usize,
}
impl Gateway {
    fn charge(&mut self, bytes: usize, add_node: bool) -> DomResult<()> {
        if bytes > MAX_STRING_BYTES
            || self.mutation_bytes.saturating_add(bytes) > MAX_MUTATION_BYTES
            || (add_node
                && self
                    .document
                    .borrow()
                    .nodes
                    .len()
                    .saturating_sub(self.initial_nodes)
                    >= MAX_ADDED_NODES)
        {
            return Err(failure(
                "QuotaExceededError",
                "DOM mutation budget exceeded",
            ));
        }
        self.mutation_bytes += bytes;
        Ok(())
    }

    fn call(&mut self, request: &Value) -> DomResult<Value> {
        let args = request
            .as_array()
            .ok_or_else(|| failure("TypeError", "Expected a DOM request"))?;
        let operation = input(args, 0)?;
        if operation == "jquery" {
            return jquery::call(self, args);
        }
        if operation == "root" {
            return Ok(json!(self.document.borrow().root));
        }
        if operation == "window" {
            return Ok(self.window.as_ref().map_or(Value::Null, |window| {
                let metrics = window.get();
                json!({"innerWidth": metrics.inner_width, "innerHeight": metrics.inner_height})
            }));
        }
        let id = args
            .get(1)
            .and_then(Value::as_u64)
            .and_then(|id| usize::try_from(id).ok())
            .filter(|&id| id < self.document.borrow().nodes.len())
            .ok_or_else(|| failure("TypeError", "Invalid node handle"))?;
        match operation {
            "describe" => {
                let doc = self.document.borrow();
                let kind = &doc.nodes[id].kind;
                let (node_type, name) = match kind {
                    NodeKind::Document => (9, "#document".into()),
                    NodeKind::Fragment => (11, "#document-fragment".into()),
                    NodeKind::Element {
                        name, namespace, ..
                    } => (
                        1,
                        if *namespace == Namespace::Html {
                            name.to_ascii_uppercase()
                        } else {
                            name.clone()
                        },
                    ),
                    NodeKind::Text(_) => (3, "#text".into()),
                    NodeKind::Comment(_) => (8, "#comment".into()),
                    NodeKind::Doctype { name, .. } => (10, name.clone()),
                };
                Ok(json!({"type":node_type,"name":name}))
            }
            "get" => {
                let property = input(args, 2)?;
                let doc = self.document.borrow();
                let node = &doc.nodes[id];
                Ok(match property {
                    "currentScript" => {
                        if !matches!(node.kind, NodeKind::Document) {
                            return Err(failure("TypeError", "Expected a Document"));
                        }
                        json!(self.current_script.get())
                    }
                    "textContent" => match &node.kind {
                        NodeKind::Document | NodeKind::Doctype { .. } => Value::Null,
                        NodeKind::Text(value) | NodeKind::Comment(value) => json!(value),
                        _ => json!(doc.text_content(id)),
                    },
                    "nodeValue" => match &node.kind {
                        NodeKind::Text(value) | NodeKind::Comment(value) => json!(value),
                        _ => Value::Null,
                    },
                    "parentNode" => json!(node.parent),
                    "parentElement" => json!(node.parent.filter(|&parent| matches!(
                        doc.nodes[parent].kind,
                        NodeKind::Element { .. }
                    ))),
                    "ownerDocument" => {
                        if matches!(node.kind, NodeKind::Document) {
                            Value::Null
                        } else {
                            json!(doc.root)
                        }
                    }
                    "firstChild" => json!(node.children.first()),
                    "lastChild" => json!(node.children.last()),
                    "nextSibling" | "previousSibling" => {
                        let sibling = node.parent.and_then(|parent| {
                            let children = &doc.nodes[parent].children;
                            let index = children.iter().position(|&child| child == id)?;
                            let next = if property == "nextSibling" {
                                index.checked_add(1)
                            } else {
                                index.checked_sub(1)
                            }?;
                            children.get(next).copied()
                        });
                        json!(sibling)
                    }
                    "isConnected" => {
                        let mut current = id;
                        while let Some(parent) = doc.nodes[current].parent {
                            current = parent;
                        }
                        json!(matches!(doc.nodes[current].kind, NodeKind::Document))
                    }
                    "localName" => doc
                        .element(id)
                        .map_or(Value::Null, |(name, _, _)| json!(name)),
                    "namespaceURI" => doc
                        .element(id)
                        .map_or(Value::Null, |(_, namespace, _)| json!(namespace.uri())),
                    "documentElement" => json!(
                        doc.nodes[doc.root]
                            .children
                            .iter()
                            .copied()
                            .find(|&child| matches!(
                                doc.nodes[child].kind,
                                NodeKind::Element { .. }
                            ))
                    ),
                    "head" | "body" => {
                        let html = doc.nodes[doc.root].children.iter().copied().find(|&child| {
                            doc.name(child) == "html" && doc.namespace(child) == Namespace::Html
                        });
                        json!(
                            html.and_then(|html| doc.nodes[html].children.iter().copied().find(
                                |&child| doc.namespace(child) == Namespace::Html
                                    && (doc.name(child) == property
                                        || (property == "body" && doc.name(child) == "frameset"))
                            ))
                        )
                    }
                    _ => return Err(failure("NotSupportedError", "Unsupported DOM property")),
                })
            }
            "set" => {
                let property = input(args, 2)?;
                let value = input(args, 3)?;
                if !matches!(property, "textContent" | "nodeValue") {
                    return Err(failure("NotSupportedError", "Unsupported DOM setter"));
                }
                let replaces_children = {
                    let doc = self.document.borrow();
                    match &doc.nodes[id].kind {
                        NodeKind::Document | NodeKind::Doctype { .. } => return Ok(Value::Null),
                        NodeKind::Element { .. } | NodeKind::Fragment
                            if property == "nodeValue" =>
                        {
                            return Ok(Value::Null);
                        }
                        NodeKind::Element { .. } | NodeKind::Fragment => true,
                        _ => false,
                    }
                };
                self.charge(value.len(), replaces_children && !value.is_empty())?;
                let mut doc = self.document.borrow_mut();
                if replaces_children {
                    for child in std::mem::take(&mut doc.nodes[id].children) {
                        doc.nodes[child].parent = None;
                    }
                    if !value.is_empty() {
                        let child = doc.add(NodeKind::Text(value.into()));
                        doc.append(id, child);
                    }
                } else if let NodeKind::Text(text) | NodeKind::Comment(text) =
                    &mut doc.nodes[id].kind
                {
                    *text = value.into();
                }
                Ok(Value::Null)
            }
            "attribute" => {
                let action = input(args, 2)?;
                let raw_name = input(args, 3)?;
                let name = {
                    let doc = self.document.borrow();
                    let (_, namespace, _) = doc
                        .element(id)
                        .ok_or_else(|| failure("TypeError", "Expected an Element"))?;
                    if namespace == Namespace::Html {
                        raw_name.to_ascii_lowercase()
                    } else {
                        raw_name.into()
                    }
                };
                if action == "set" {
                    if raw_name.is_empty()
                        || raw_name.chars().any(|ch| {
                            matches!(
                                ch,
                                '\t' | '\n' | '\x0c' | '\r' | ' ' | '\0' | '/' | '=' | '>'
                            )
                        })
                    {
                        return Err(failure("InvalidCharacterError", "Invalid attribute name"));
                    }
                    let value = input(args, 4)?;
                    self.charge(name.len().saturating_add(value.len()), false)?;
                    let mut doc = self.document.borrow_mut();
                    let NodeKind::Element { attrs, .. } = &mut doc.nodes[id].kind else {
                        unreachable!()
                    };
                    if let Some(attribute) =
                        attrs.iter_mut().find(|attr| qualified_name(attr) == name)
                    {
                        attribute.value = value.into();
                    } else {
                        attrs.push(Attribute {
                            name,
                            value: value.into(),
                            namespace: None,
                            prefix: None,
                        });
                    }
                    return Ok(Value::Null);
                }
                let mut doc = self.document.borrow_mut();
                let NodeKind::Element { attrs, .. } = &mut doc.nodes[id].kind else {
                    unreachable!()
                };
                let position = attrs
                    .iter()
                    .position(|attribute| qualified_name(attribute) == name);
                Ok(match action {
                    "get" => position.map_or(Value::Null, |index| json!(attrs[index].value)),
                    "has" => json!(position.is_some()),
                    "remove" => {
                        if let Some(index) = position {
                            attrs.remove(index);
                        }
                        Value::Null
                    }
                    _ => {
                        return Err(failure(
                            "NotSupportedError",
                            "Unsupported attribute operation",
                        ));
                    }
                })
            }
            "id" => {
                let target = input(args, 2)?;
                let doc = self.document.borrow();
                if target.is_empty() {
                    return Ok(Value::Null);
                }
                let mut pending = doc.nodes[id]
                    .children
                    .iter()
                    .rev()
                    .copied()
                    .collect::<Vec<_>>();
                while let Some(child) = pending.pop() {
                    if doc.element(child).is_some_and(|(_, _, attrs)| {
                        attrs.iter().any(|attr| {
                            attr.namespace.is_none() && attr.name == "id" && attr.value == target
                        })
                    }) {
                        return Ok(json!(child));
                    }
                    pending.extend(doc.nodes[child].children.iter().rev().copied());
                }
                Ok(Value::Null)
            }
            "query" => {
                let selector = input(args, 2)?;
                if selector.len() > MAX_STRING_BYTES {
                    return Err(failure(
                        "QuotaExceededError",
                        "Selector exceeds the DOM string budget",
                    ));
                }
                let document = self.document.borrow();
                let ids = super::selectors::query_selector_ids(&document, id, selector).map_err(
                    |message| {
                        let (name, detail) = message
                            .split_once(": ")
                            .unwrap_or(("SyntaxError", &message));
                        failure(
                            match name {
                                "NotSupportedError" => "NotSupportedError",
                                "QuotaExceededError" => "QuotaExceededError",
                                _ => "SyntaxError",
                            },
                            detail,
                        )
                    },
                )?;
                if ids.len() > MAX_QUERY_RESULTS {
                    return Err(failure(
                        "QuotaExceededError",
                        "Selector result exceeds the DOM result budget",
                    ));
                }
                Ok(json!(ids))
            }
            _ => Err(failure("NotSupportedError", "Unsupported DOM operation")),
        }
    }
}
fn qualified_name(attribute: &Attribute) -> String {
    match &attribute.prefix {
        Some(prefix) => format!("{prefix}:{}", attribute.name),
        None => attribute.name.clone(),
    }
}
