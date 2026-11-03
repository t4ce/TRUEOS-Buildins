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
#[path = "javascript_dom_mutation.rs"]
mod mutation;
#[path="javascript_style.rs"]
mod style;
#[path = "javascript_url.rs"]
mod url_api;
#[path = "javascript_cookies.rs"]
pub(super) mod cookies;
#[path="javascript_timers.rs"]
pub(super) mod timers;
#[path="javascript_media.rs"]
pub(super) mod media;

pub(super) fn install_url_with_timers(engine: &mut Engine, timers:Option<timers::SharedTimers>)->Result<(),String>{url_api::install_with_timers(engine,timers)}
pub(super) fn install_url_only(engine: &mut Engine) -> Result<(), String> {
    url_api::install(engine)
}

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
    install_gateway(
        engine,
        document,
        None,
        Rc::new(Cell::new(None)),
        "about:blank",
        Rc::new(RefCell::new(cookies::CookieJar::default())),
        None,
        None,
    )
}

/// Window metrics come from Hull's content viewport, not native frame guesses.
pub(super) fn install_with_window(
    engine: &mut Engine,
    document: SharedDocument,
    window: SharedWindowMetrics,
) -> Result<(), String> {
    install_gateway(
        engine,
        document,
        Some(window),
        Rc::new(Cell::new(None)),
        "about:blank",
        Rc::new(RefCell::new(cookies::CookieJar::default())),
        None,
        None,
    )
}

/// The actor owns script execution scope; the gateway only reads its handle.
pub(super) fn install_for_page(
    engine: &mut Engine,
    document: SharedDocument,
    window: Option<SharedWindowMetrics>,
    current_script: SharedCurrentScript,
    page_url: &str,
) -> Result<(), String> {
    install_gateway(engine, document, window, current_script, page_url, Rc::new(RefCell::new(cookies::CookieJar::default())),None,None)
}

pub(super) fn install_for_page_with_cookies(
    engine: &mut Engine,
    document: SharedDocument,
    window: Option<SharedWindowMetrics>,
    current_script: SharedCurrentScript,
    page_url: &str,
    cookies: cookies::SharedCookies,
    timers:Option<timers::SharedTimers>,
    media:Option<media::SharedMedia>,
) -> Result<(), String> {
    install_gateway(engine, document, window, current_script, page_url, cookies,timers,media)
}

fn install_gateway(
    engine: &mut Engine,
    document: SharedDocument,
    window: Option<SharedWindowMetrics>,
    current_script: SharedCurrentScript,
    page_url: &str,
    cookies: cookies::SharedCookies,
    timers:Option<timers::SharedTimers>,
    media:Option<media::SharedMedia>,
) -> Result<(), String> {
    let initial_nodes = document.borrow().nodes.len();
    let mut gateway = Gateway {
        document,
        window,
        current_script,
        page_url: page_url.to_owned(),
        cookies,
        timers,
        media,
        style_locks:Default::default(),
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
    let source = include_str!("javascript_dom.js")
        .replace("/* HULL_COOKIES */", include_str!("javascript_cookies.js"))
        .replace("/* HULL_STYLE */", include_str!("javascript_style.js"))
        .replace("/* HULL_MEDIA */", include_str!("javascript_media.js"))
        .replace(
            "/* HULL_DOM_EVENTS */",
            include_str!("javascript_event_target.js"),
        )
        .replace(
            "/* HULL_INPUT_EVENTS */",
            include_str!("javascript_events.js"),
        )
        .replace(
            "/* HULL_NATIVE_JQUERY */",
            include_str!("javascript_jquery.js"),
        )
        .replace(
            "/* HULL_NATIVE_URL */",
            &format!("({})(request);", include_str!("javascript_url.js")),
        )
        .replace("/* HULL_NAVIGATION */", include_str!("javascript_navigation.js"))
        .replace("/* HULL_NAVIGATOR */", include_str!("javascript_navigator.js"));
    engine.eval(source.as_bytes(), "hull:dom", false)?;
    engine.bind_message_handler("__hullInputReceive")
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
    style_locks:style::Locks,
    timers:Option<timers::SharedTimers>,
    media:Option<media::SharedMedia>,
    cookies: cookies::SharedCookies,
    document: SharedDocument,
    window: Option<SharedWindowMetrics>,
    current_script: SharedCurrentScript,
    page_url: String,
    initial_nodes: usize,
    mutation_bytes: usize,
}
impl Gateway {
    fn charge(&mut self, bytes: usize, add_node: bool) -> DomResult<()> {
        self.charge_nodes(bytes, usize::from(add_node))
    }
    fn charge_nodes(&mut self, bytes: usize, add_nodes: usize) -> DomResult<()> {
        if bytes > MAX_STRING_BYTES
            || self.mutation_bytes.saturating_add(bytes) > MAX_MUTATION_BYTES
            || (add_nodes > 0
                && self
                    .document
                    .borrow()
                    .nodes
                    .len()
                    .saturating_sub(self.initial_nodes)
                    .saturating_add(add_nodes)
                    > MAX_ADDED_NODES)
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
        if operation=="timers" {return Ok(json!(self.timers.is_some()));}
        if operation=="mediaPlay" {
            let node=args.get(1).and_then(Value::as_u64).and_then(|v|usize::try_from(v).ok()).ok_or_else(||failure("TypeError","Expected video node"))?;
            if !self.document.borrow().element(node).is_some_and(|(name,ns,_)|name=="video"&&ns==Namespace::Html){return Err(failure("TypeError","Expected HTMLVideoElement"));}
            let address=input(args,2)?;
            let url=url::Url::parse(address).map_err(|_|failure("NotSupportedError","Invalid media URL"))?;
            if url.scheme()!="https"||url.host_str().is_none()||url.username()!=""||url.password().is_some()||address.len()>3072{return Err(failure("NotSupportedError","Only bounded HTTPS media URLs are admitted"));}
            let media=self.media.as_ref().ok_or_else(||failure("NotSupportedError","Media handoff is not installed"))?;
            return Ok(json!(media.borrow_mut().request(node,url.to_string())));
        }
        if operation=="timer" {
            return self.timers.as_ref().map(|state|timers::call(state,&args[1..])).ok_or_else(||failure("NotSupportedError","Timers are not installed"));
        }
        if operation == "url" {
            return url_api::call(args);
        }
        if operation == "jquery" {
            return jquery::call(self, args);
        }
        if operation == "cookieFull" {
            self.cookies.borrow_mut().ignore_further_writes();
            return Ok(Value::Null);
        }
        if operation=="sessionStorage" {
            return self.cookies.borrow_mut().session_storage(input(args,1)?,args.get(2).and_then(Value::as_str).unwrap_or(""),args.get(3).and_then(Value::as_str).unwrap_or(""))
                .ok_or_else(||failure("NotSupportedError","Unsupported storage operation"));
        }
        if operation == "storage" {
            return self.cookies.borrow_mut().storage(input(args,1)?,args.get(2).and_then(Value::as_str).unwrap_or(""),args.get(3).and_then(Value::as_str).unwrap_or(""))
                .ok_or_else(||failure("NotSupportedError","Unsupported storage operation"));
        }
        if operation == "cookie" {
            if let Some(value) = args.get(1).and_then(Value::as_str) { self.cookies.borrow_mut().set(value); return Ok(Value::Null); }
            return Ok(json!(self.cookies.borrow().get()));
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
        if matches!(
            operation,
            "create" | "appendChild" | "insertBefore" | "removeChild" | "clone"
        ) {
            return mutation::call(self, operation, id, args);
        }
        if operation=="style" {return style::call(self,id,input(args,2)?,args.get(3).and_then(Value::as_str).unwrap_or(""),args.get(4).and_then(Value::as_str).unwrap_or(""),args.get(5).and_then(Value::as_str).unwrap_or(""));}
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
                let namespace = doc.element(id).map(|(_, ns, _)| ns.uri());
                Ok(json!({"type":node_type,"name":name,"namespace":namespace}))
            }
            "get" => {
                let property = input(args, 2)?;
                let doc = self.document.borrow();
                let node = &doc.nodes[id];
                Ok(match property {
                    "baseURI" => json!(mutation::base_uri(&doc, &self.page_url)?),
                    "URL" | "documentURI" => {
                        if !matches!(node.kind, NodeKind::Document) {
                            return Err(failure("TypeError", "Expected a Document"));
                        }
                        json!(self.page_url)
                    }
                    "linkHref" => {
                        if !doc
                            .element(id)
                            .is_some_and(|(name, ns, _)| name == "link" && ns == Namespace::Html)
                        {
                            return Err(failure("TypeError", "Expected an HTMLLinkElement"));
                        }
                        let value = doc
                            .element(id)
                            .unwrap()
                            .2
                            .iter()
                            .find(|a| a.namespace.is_none() && a.name == "href")
                            .map(|a| a.value.as_str());
                        json!(match value {
                            None => String::new(),
                            Some(value) =>
                                url::Url::parse(&mutation::base_uri(&doc, &self.page_url)?)
                                    .and_then(|base| base.join(value))
                                    .map_or_else(|_| value.to_owned(), |u| u.to_string()),
                        })
                    }
                    "templateContent" => {
                        let NodeKind::Element {
                            name,
                            namespace: Namespace::Html,
                            template_content: Some(fragment),
                            ..
                        } = &node.kind
                        else {
                            return Err(failure("TypeError", "Expected an HTMLTemplateElement"));
                        };
                        if name != "template" {
                            return Err(failure("TypeError", "Expected an HTMLTemplateElement"));
                        }
                        json!(fragment)
                    }
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
                if name=="style"&&(action=="set"||action=="remove")&&self.style_locks.borrow().iter().any(|(node,_)|*node==id){
                    return style::call(self,id,"cssText","",if action=="set"{input(args,4)?}else{""},"");
                }
                let style_fields=if name=="style"&&action=="set"{style::attribute_locks(self,id,input(args,4)?)?}
                    else if name=="style"&&action=="remove"{
                        let previous=self.document.borrow().element(id).unwrap().2.iter().find(|a|a.name=="style"&&a.namespace.is_none()).map(|a|a.value.clone()).unwrap_or_default();
                        style::attribute_locks(self,id,&previous)?
                    }else{Vec::new()};
                if action == "set" || action=="styleCommit" {
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
                    self.style_locks.borrow_mut().extend(style_fields);
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
                        self.style_locks.borrow_mut().extend(style_fields);
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
            "tags" => {
                let requested = input(args, 2)?;
                if requested.len() > MAX_STRING_BYTES {
                    return Err(failure(
                        "QuotaExceededError",
                        "Tag name exceeds the DOM string budget",
                    ));
                }
                let document = self.document.borrow();
                if document.nodes.len() > 100_000 {
                    return Err(failure(
                        "QuotaExceededError",
                        "DOM tree exceeds collection bounds",
                    ));
                }
                let html_name = requested.to_ascii_lowercase();
                let wildcard = requested == "*";
                let mut pending = document.nodes[id]
                    .children
                    .iter()
                    .rev()
                    .copied()
                    .collect::<Vec<_>>();
                let mut ids = Vec::new();
                let until = std::time::Instant::now() + std::time::Duration::from_millis(20);
                let mut visited = 0usize;
                while let Some(candidate) = pending.pop() {
                    visited += 1;
                    if visited % 256 == 0 && std::time::Instant::now() >= until {
                        return Err(failure(
                            "QuotaExceededError",
                            "Tag collection traversal budget exhausted",
                        ));
                    }
                    if let Some((name, namespace, _)) = document.element(candidate) {
                        if wildcard
                            || if namespace == Namespace::Html {
                                name.eq_ignore_ascii_case(&html_name)
                            } else {
                                name == requested
                            }
                        {
                            if ids.len() == MAX_QUERY_RESULTS {
                                return Err(failure(
                                    "QuotaExceededError",
                                    "Tag collection result exceeds the DOM result budget",
                                ));
                            }
                            ids.push(candidate);
                        }
                    }
                    pending.extend(document.nodes[candidate].children.iter().rev().copied());
                }
                if std::time::Instant::now() >= until {
                    return Err(failure(
                        "QuotaExceededError",
                        "Tag collection traversal budget exhausted",
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
