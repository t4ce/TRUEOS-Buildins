//! Fragment parsing and serialization use the same rp5 arena as ordinary nodes.
#[cfg(test)]
use alloc::vec;
use alloc::{string::String, vec::Vec};
use super::{DomResult, Gateway, MAX_STRING_BYTES, failure};
use rp5::{Document, Namespace, NodeKind};

pub(super) fn set(gateway: &mut Gateway, id: usize, text: &str) -> DomResult<()> {
    if text.len() > MAX_STRING_BYTES { return Err(failure("QuotaExceededError", "HTML fragment string budget exceeded")); }
    let context = {
        let doc = gateway.document.borrow();
        let (name, namespace, attrs) = doc.element(id).ok_or_else(|| failure("TypeError", "Expected an Element"))?;
        rp5::FragmentContext { name: name.into(), namespace, attrs: attrs.to_vec() }
    };
    let fragment = rp5::parse_fragment(text, Some(context));
    gateway.charge_nodes(text.len(), fragment.nodes.len())?;
    let mut doc = gateway.document.borrow_mut();
    let target = match &doc.nodes[id].kind {
        NodeKind::Element { template_content: Some(content), .. } => *content,
        _ => id,
    };
    let offset = doc.nodes.len();
    for node in &fragment.nodes {
        let mut kind = node.kind.clone();
        if let NodeKind::Element { template_content: Some(content), .. } = &mut kind { *content += offset; }
        let new_id = doc.add(kind);
        // HTML fragment insertion never executes its script elements, even if
        // subsequently moved into the live page.
        if doc.name(new_id) == "script" { gateway.script_state.borrow_mut().started.insert(new_id); }
    }
    for (index, node) in fragment.nodes.iter().enumerate() {
        doc.nodes[offset + index].parent = node.parent.map(|p| p + offset);
        doc.nodes[offset + index].children = node.children.iter().map(|c| c + offset).collect();
    }
    for child in core::mem::take(&mut doc.nodes[target].children) { doc.nodes[child].parent = None; }
    let children = core::mem::take(&mut doc.nodes[offset + fragment.root].children);
    for child in children { doc.nodes[child].parent = Some(target); doc.nodes[target].children.push(child); }
    Ok(())
}
fn escaped(value: &str, attribute: bool) -> String {
    let mut out = value.replace('&', "&amp;").replace('\u{a0}', "&nbsp;");
    if attribute { out = out.replace('"', "&quot;"); }
    else { out = out.replace('<', "&lt;").replace('>', "&gt;"); }
    out
}
pub(super) fn get(doc: &Document, id: usize) -> DomResult<String> {
    if doc.element(id).is_none() { return Err(failure("TypeError", "Expected an Element")); }
    let root = match doc.nodes[id].kind { NodeKind::Element { template_content: Some(content), .. } => content, _ => id };
    let mut out = String::new();
    let mut pending: Vec<(usize, bool)> = doc.nodes[root].children.iter().rev().map(|&n| (n,false)).collect();
    while let Some((node, closing)) = pending.pop() {
        match &doc.nodes[node].kind {
            NodeKind::Element { name, namespace, attrs, template_content } => {
                if closing { out.push_str("</"); out.push_str(name); out.push('>'); continue; }
                out.push('<'); out.push_str(name);
                for a in attrs {
                    out.push(' ');
                    if let Some(prefix) = &a.prefix { out.push_str(prefix); out.push(':'); }
                    out.push_str(&a.name); out.push_str("=\""); out.push_str(&escaped(&a.value,true)); out.push('"');
                }
                out.push('>');
                if *namespace == Namespace::Html && matches!(name.as_str(),"area"|"base"|"basefont"|"bgsound"|"link"|"meta"|"img"|"embed"|"param"|"hr"|"input"|"keygen"|"source"|"track"|"wbr"|"br"|"col") { continue; }
                pending.push((node,true));
                let parent = template_content.unwrap_or(node);
                pending.extend(doc.nodes[parent].children.iter().rev().map(|&n|(n,false)));
            }
            NodeKind::Text(text) => {
                let raw = doc.nodes[node].parent.is_some_and(|p| doc.namespace(p)==Namespace::Html && matches!(doc.name(p),"style"|"script"|"xmp"|"iframe"|"noembed"|"noframes"|"plaintext"));
                if raw { out.push_str(text); } else { out.push_str(&escaped(text,false)); }
            }
            NodeKind::Comment(text) => { out.push_str("<!--"); out.push_str(text); out.push_str("-->"); }
            NodeKind::Doctype { name, .. } => { out.push_str("<!DOCTYPE "); out.push_str(name); out.push('>'); }
            _ => {}
        }
        if out.len() > MAX_STRING_BYTES { return Err(failure("QuotaExceededError", "HTML serialization budget exceeded")); }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::{Cell,RefCell};
    use alloc::rc::Rc;
    #[test]
    fn over_node_budget_fragment_leaves_existing_tree_untouched() {
        let doc=rp5::parse("<body>keep</body>");
        let root=doc.root;let count=doc.nodes.len();
        let body=doc.nodes.iter().enumerate().find(|(n,_)|doc.name(*n)=="body").unwrap().0;
        let mut gateway=Gateway {
            script_state:Default::default(), documents:Rc::new(RefCell::new(vec![root])),
            owners:Rc::new(RefCell::new(vec![root;count])), style_locks:Default::default(),
            timers:None,media:None,cookies:Default::default(),document:Rc::new(RefCell::new(doc)),
            window:None,current_script:Rc::new(Cell::new(None)),page_url:"https://fixture.test/".into(),
            initial_nodes:count,mutation_bytes:0,
        };
        let error=set(&mut gateway,body,&"<!--x-->".repeat(4100)).unwrap_err();
        assert_eq!(error.0,"QuotaExceededError");
        assert_eq!(gateway.document.borrow().nodes.len(),count);
        assert_eq!(gateway.document.borrow().text_content(body),"keep");
        assert_eq!(gateway.mutation_bytes,0);
    }
}
