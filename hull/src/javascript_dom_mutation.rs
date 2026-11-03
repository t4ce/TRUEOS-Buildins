//! Bounded native node creation and pre-insertion over the existing rp5 arena.
use super::{DomResult, Gateway, MAX_QUERY_RESULTS, MAX_STRING_BYTES, failure, input};
use rp5::{Document, Namespace, NodeKind};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn hierarchy() -> (&'static str, String) {
    failure(
        "HierarchyRequestError",
        "Invalid parent/child tree relationship",
    )
}
fn handle(gateway: &Gateway, args: &[Value], index: usize) -> DomResult<usize> {
    args.get(index)
        .and_then(Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .filter(|id| *id < gateway.document.borrow().nodes.len())
        .ok_or_else(|| failure("TypeError", "Invalid node handle"))
}
fn valid_name(name: &str) -> bool {
    // DOM's valid-element-local-name algorithm, not a CSS selector or markup.
    let Some(first) = name.chars().next() else {
        return false;
    };
    if first.is_ascii_alphabetic() {
        return !name
            .chars()
            .any(|c| matches!(c, '\0' | '\t' | '\n' | '\x0c' | '\r' | ' ' | '/' | '>'));
    }
    (matches!(first, ':' | '_') || first as u32 >= 128)
        && name.chars().skip(1).all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '_') || c as u32 >= 128
        })
}
pub(super) fn base_uri(doc: &Document, page_url: &str, root: usize) -> DomResult<String> {
    if doc.nodes.len() > 100_000 {
        return Err(failure(
            "QuotaExceededError",
            "DOM tree exceeds base URI bounds",
        ));
    }
    let until = Instant::now() + Duration::from_millis(20);
    let page_url = if root == doc.root { page_url } else { "about:blank" };
    let mut pending = vec![root];
    let mut visited = 0;
    while let Some(id) = pending.pop() {
        visited += 1;
        if visited % 256 == 0 && Instant::now() >= until {
            return Err(failure(
                "QuotaExceededError",
                "Base URI traversal budget exceeded",
            ));
        }
        if let Some(("base", Namespace::Html, attrs)) = doc.element(id) {
            if let Some(a) = attrs
                .iter()
                .find(|a| a.namespace.is_none() && a.name == "href")
            {
                return Ok(url::Url::parse(page_url)
                    .and_then(|u| u.join(&a.value))
                    .ok()
                    .filter(|u| !matches!(u.scheme(), "javascript" | "data"))
                    .map_or_else(|| page_url.to_owned(), |u| u.to_string()));
            }
        }
        pending.extend(doc.nodes[id].children.iter().rev().copied());
    }
    Ok(page_url.to_owned())
}
pub(super) fn call(
    gateway: &mut Gateway,
    operation: &str,
    id: usize,
    args: &[Value],
) -> DomResult<Value> {
    if operation == "clone" {
        return clone_node(
            gateway,
            id,
            args.get(2).and_then(Value::as_bool).unwrap_or(false),
        );
    }
    if operation == "create" {
        if !matches!(gateway.document.borrow().nodes[id].kind, NodeKind::Document) {
            return Err(failure("TypeError", "Expected a Document"));
        }
        let kind = input(args, 2)?;
        let value = if kind == "fragment" {
            ""
        } else {
            input(args, 3)?
        };
        if value.len() > MAX_STRING_BYTES {
            return Err(failure("QuotaExceededError", "DOM string budget exceeded"));
        }
        let node = match kind {
            "element" => {
                if !valid_name(value) {
                    return Err(failure(
                        "InvalidCharacterError",
                        "Invalid element local name",
                    ));
                }
                let name = value.to_ascii_lowercase();
                NodeKind::Element {
                    name,
                    namespace: Namespace::Html,
                    attrs: Vec::new(),
                    template_content: None,
                }
            }
            "text" => NodeKind::Text(value.into()),
            "comment" => NodeKind::Comment(value.into()),
            "fragment" => NodeKind::Fragment,
            _ => {
                return Err(failure(
                    "NotSupportedError",
                    "Unsupported node creation operation",
                ));
            }
        };
        let template = matches!(&node,NodeKind::Element{name,..} if name=="template");
        gateway.charge_nodes(value.len(), if template { 2 } else { 1 })?;
        let mut doc = gateway.document.borrow_mut();
        let id = doc.add(node);
        if template {
            let fragment = doc.add(NodeKind::Fragment);
            if let NodeKind::Element {
                template_content, ..
            } = &mut doc.nodes[id].kind
            {
                *template_content = Some(fragment);
            }
        }
        return Ok(json!(id));
    }
    let child = handle(gateway, args, 2)?;
    if operation == "removeChild" {
        if gateway.document.borrow().nodes[child].parent != Some(id) {
            return Err(failure(
                "NotFoundError",
                "Node is not a child of this parent",
            ));
        }
        gateway.charge(0, false)?;
        gateway.document.borrow_mut().detach(child);
        return Ok(json!(child));
    }
    let before = if operation == "insertBefore" && !args.get(3).is_none_or(Value::is_null) {
        Some(handle(gateway, args, 3)?)
    } else {
        None
    };
    let doc = gateway.document.borrow();
    if !matches!(
        doc.nodes[id].kind,
        NodeKind::Document | NodeKind::Fragment | NodeKind::Element { .. }
    ) {
        return Err(hierarchy());
    }
    let until = Instant::now() + Duration::from_millis(20);
    let mut parent = Some(id);
    let mut steps = 0;
    let mut template_hosts = None;
    while let Some(current) = parent {
        if current == child {
            return Err(hierarchy());
        }
        steps += 1;
        if steps > 100_000 || steps % 256 == 0 && Instant::now() >= until {
            return Err(failure(
                "QuotaExceededError",
                "Insertion ancestry budget exceeded",
            ));
        }
        parent = doc.nodes[current].parent;
        if parent.is_none() && matches!(doc.nodes[current].kind, NodeKind::Fragment) {
            if doc.nodes.len() > 100_000 {
                return Err(failure(
                    "QuotaExceededError",
                    "Template ancestry budget exceeded",
                ));
            }
            let hosts = template_hosts.get_or_insert_with(|| {
                doc.nodes
                    .iter()
                    .enumerate()
                    .filter_map(|(id, node)| {
                        if let NodeKind::Element {
                            name,
                            namespace: Namespace::Html,
                            template_content: Some(fragment),
                            ..
                        } = &node.kind
                        {
                            (name == "template").then_some((*fragment, id))
                        } else {
                            None
                        }
                    })
                    .collect::<std::collections::HashMap<_, _>>()
            });
            parent = hosts.get(&current).copied();
        }
    }
    if before.is_some_and(|node| doc.nodes[node].parent != Some(id)) {
        return Err(failure(
            "NotFoundError",
            "Reference node is not a child of this parent",
        ));
    }
    if matches!(doc.nodes[child].kind, NodeKind::Document) {
        return Err(hierarchy());
    }
    if matches!(doc.nodes[child].kind, NodeKind::Doctype { .. })
        && !matches!(doc.nodes[id].kind, NodeKind::Document)
    {
        return Err(hierarchy());
    }
    let moving = if matches!(doc.nodes[child].kind, NodeKind::Fragment) {
        doc.nodes[child].children.clone()
    } else {
        vec![child]
    };
    if moving.len() > MAX_QUERY_RESULTS || doc.nodes[id].children.len() > 100_000 {
        return Err(failure(
            "QuotaExceededError",
            "Insertion child count exceeds work budget",
        ));
    }
    let before = if before == Some(child) {
        doc.nodes[child].parent.and_then(|parent| {
            let children = &doc.nodes[parent].children;
            let at = children.iter().position(|n| *n == child)?;
            children.get(at + 1).copied()
        })
    } else {
        before
    };
    if matches!(doc.nodes[id].kind, NodeKind::Document) {
        let moved = moving
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let mut final_children = doc.nodes[id]
            .children
            .iter()
            .copied()
            .filter(|node| !moved.contains(node))
            .collect::<Vec<_>>();
        let position = before
            .and_then(|node| final_children.iter().position(|n| *n == node))
            .unwrap_or(final_children.len());
        final_children.splice(position..position, moving.iter().copied());
        let mut element = false;
        let mut doctype = false;
        for node in final_children {
            match doc.nodes[node].kind {
                NodeKind::Text(_) | NodeKind::Document | NodeKind::Fragment => {
                    return Err(hierarchy());
                }
                NodeKind::Element { .. } => {
                    if element {
                        return Err(hierarchy());
                    }
                    element = true;
                }
                NodeKind::Doctype { .. } => {
                    if doctype || element {
                        return Err(hierarchy());
                    }
                    doctype = true;
                }
                _ => {}
            }
        }
    }
    if Instant::now() >= until {
        return Err(failure(
            "QuotaExceededError",
            "Insertion validation budget exceeded",
        ));
    }
    drop(doc);
    gateway.charge(0, false)?;
    let mut doc = gateway.document.borrow_mut();
    // Keep validation atomic. After detaching once, splice once rather than
    // repeatedly scanning/removing sibling lists for fragment children.
    let mut moved = std::collections::HashSet::new();
    moved.extend(moving.iter().copied());
    let mut old_parents = std::collections::HashSet::new();
    for node in &moving {
        if let Some(parent) = doc.nodes[*node].parent {
            old_parents.insert(parent);
        }
    }
    for parent in old_parents {
        doc.nodes[parent]
            .children
            .retain(|node| !moved.contains(node));
    }
    let position = before
        .and_then(|node| doc.nodes[id].children.iter().position(|n| *n == node))
        .unwrap_or(doc.nodes[id].children.len());
    doc.nodes[id]
        .children
        .splice(position..position, moving.iter().copied());
    for node in moving {
        doc.nodes[node].parent = Some(id);
    }
    Ok(json!(child))
}

// Plan first: a rejected clone must not leave a partly allocated subtree.
// This is iterative for both ordinary descendants and template contents.
fn clone_node(gateway: &mut Gateway, id: usize, deep: bool) -> DomResult<Value> {
    let doc = gateway.document.borrow();
    if matches!(doc.nodes[id].kind, NodeKind::Document) {
        return Err(failure(
            "NotSupportedError",
            "Cloning Documents requires a separate document realm",
        ));
    }
    let until = Instant::now() + Duration::from_millis(20);
    let mut pending = vec![(id, None, false)];
    let mut plan = Vec::new();
    let mut script_started = Vec::new();
    let mut bytes = 0usize;
    while let Some((source, parent, template_root)) = pending.pop() {
        if plan.len() + pending.len() >= super::MAX_ADDED_NODES || Instant::now() >= until {
            return Err(failure(
                "QuotaExceededError",
                "DOM clone work budget exceeded",
            ));
        }
        let node = &doc.nodes[source];
        let size = match &node.kind {
            NodeKind::Element { name, attrs, .. } => {
                name.len()
                    + attrs
                        .iter()
                        .map(|a| a.name.len() + a.value.len())
                        .sum::<usize>()
            }
            NodeKind::Text(s) | NodeKind::Comment(s) => s.len(),
            NodeKind::Doctype {
                name,
                public_id,
                system_id,
            } => name.len() + public_id.len() + system_id.len(),
            _ => 0,
        };
        bytes = bytes.saturating_add(size);
        if bytes > MAX_STRING_BYTES {
            return Err(failure(
                "QuotaExceededError",
                "DOM clone string budget exceeded",
            ));
        }
        let mut kind = node.kind.clone();
        let fragment = if let NodeKind::Element {
            template_content, ..
        } = &mut kind
        {
            template_content.take()
        } else {
            None
        };
        let index = plan.len();
        script_started.push(gateway.script_state.borrow().started.contains(&source));
        plan.push((kind, parent, template_root));
        if let Some(fragment) = fragment {
            pending.push((fragment, Some(index), true));
        }
        if deep {
            if node.children.len() + pending.len() + plan.len() > super::MAX_ADDED_NODES {
                return Err(failure(
                    "QuotaExceededError",
                    "DOM clone node budget exceeded",
                ));
            }
            pending.extend(
                node.children
                    .iter()
                    .rev()
                    .map(|child| (*child, Some(index), false)),
            );
        }
    }
    drop(doc);
    gateway.charge_nodes(bytes, plan.len())?;
    let mut doc = gateway.document.borrow_mut();
    let mut ids: Vec<usize> = Vec::with_capacity(plan.len());
    for (index, (kind, parent, template_root)) in plan.into_iter().enumerate() {
        let new_id = doc.add(kind);
        if script_started[index] { gateway.script_state.borrow_mut().started.insert(new_id); }
        if let Some(parent) = parent {
            let parent = ids[parent];
            if template_root {
                if let NodeKind::Element {
                    template_content, ..
                } = &mut doc.nodes[parent].kind
                {
                    *template_content = Some(new_id);
                }
            } else {
                doc.append(parent, new_id);
            }
        }
        ids.push(new_id);
    }
    Ok(json!(ids[0]))
}
