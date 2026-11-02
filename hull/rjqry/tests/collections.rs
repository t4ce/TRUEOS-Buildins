//! Host fixture checks collection ordering and native mutation dispatch. The
//! fixture intentionally implements only tag selectors, not a second CSS engine.
use rjqry::{Dom, Error, JQuery, NodeKind, Value};
use std::collections::BTreeMap;
#[derive(Clone)]
struct Node {
    kind: NodeKind,
    parent: Option<usize>,
    children: Vec<usize>,
    tag: &'static str,
    text: String,
    attrs: BTreeMap<String, String>,
}
struct Tree {
    nodes: Vec<Node>,
    cleaned: Vec<usize>,
    xml: bool,
    properties: Option<BTreeMap<(usize, String), Value>>,
}
impl Tree {
    fn fixture() -> Self {
        let descriptors = [
            (NodeKind::Document, None, "document", ""),
            (NodeKind::Element, Some(0), "main", ""),
            (NodeKind::Element, Some(1), "p", ""),
            (NodeKind::Text, Some(2), "", "one"),
            (NodeKind::Comment, Some(1), "", "comment"),
            (NodeKind::Element, Some(1), "div", ""),
            (NodeKind::Element, Some(5), "p", ""),
            (NodeKind::Text, Some(6), "", "two"),
            (NodeKind::Element, Some(1), "p", ""),
        ];
        let mut nodes = descriptors
            .into_iter()
            .map(|(kind, parent, tag, text)| Node {
                kind,
                parent,
                children: vec![],
                tag,
                text: text.into(),
                attrs: BTreeMap::new(),
            })
            .collect::<Vec<_>>();
        for i in 0..nodes.len() {
            if let Some(p) = nodes[i].parent {
                nodes[p].children.push(i);
            }
        }
        Self {
            nodes,
            cleaned: vec![],
            xml: false,
            properties: None,
        }
    }
    fn descendants(&self, node: usize, result: &mut Vec<usize>) {
        for child in &self.nodes[node].children {
            result.push(*child);
            self.descendants(*child, result);
        }
    }
}
impl Dom for Tree {
    type Node = usize;
    fn query(&self, context: Option<&usize>, selector: &str) -> Result<Vec<usize>, Error> {
        let context = *context.unwrap_or(&0);
        if !matches!(
            self.kind(&context),
            NodeKind::Element | NodeKind::Document | NodeKind::Fragment
        ) {
            return Err(Error::Unsupported("fixture selector context"));
        }
        let first = selector == "p:first";
        let selector = if first { "p" } else { selector };
        let mut nodes = vec![];
        self.descendants(context, &mut nodes);
        let mut result = vec![];
        for node in nodes {
            if self.matches(&node, selector)? {
                result.push(node);
            }
        }
        if first {
            result.truncate(1);
        }
        Ok(result)
    }
    fn matches(&self, node: &usize, selector: &str) -> Result<bool, Error> {
        if let Some(inner) = selector
            .strip_prefix(":not(")
            .and_then(|s| s.strip_suffix(')'))
        {
            return Ok(!self.matches(node, inner)?);
        }
        if !selector.chars().all(|c| c.is_ascii_alphabetic()) && selector != "*" {
            return Err(Error::Unsupported("fixture tag selectors only"));
        }
        Ok(self.kind(node) == NodeKind::Element
            && (selector == "*" || self.nodes[*node].tag == selector))
    }
    fn filter(&self, nodes: &[usize], selector: &str) -> Result<Vec<usize>, Error> {
        match selector {
            ":first" => return Ok(nodes.first().copied().into_iter().collect()),
            ":not(:first)" => return Ok(nodes.get(1..).unwrap_or_default().to_vec()),
            _ => {}
        }
        if rjqry::utilities::needs_context(selector) {
            return Err(Error::Unsupported("fixture seed selector"));
        }
        let mut result = vec![];
        for node in nodes {
            if self.kind(node) == NodeKind::Element && self.matches(node, selector)? {
                result.push(*node);
            }
        }
        Ok(result)
    }
    fn kind(&self, node: &usize) -> NodeKind {
        self.nodes[*node].kind
    }
    fn is_xml(&self, _: &usize) -> bool {
        self.xml
    }
    fn parent(&self, node: &usize) -> Option<usize> {
        self.nodes[*node].parent
    }
    fn children(&self, node: &usize) -> Vec<usize> {
        self.nodes[*node].children.clone()
    }
    fn unique_sort(&self, nodes: &mut Vec<usize>) {
        nodes.sort();
        nodes.dedup();
    }
    fn text(&self, node: &usize) -> String {
        let item = &self.nodes[*node];
        if item.kind == NodeKind::Text {
            return item.text.clone();
        }
        if item.kind == NodeKind::Comment {
            return String::new();
        }
        item.children.iter().map(|c| self.text(c)).collect()
    }
    fn set_text(&mut self, node: &usize, text: &str) -> Result<(), Error> {
        self.nodes[*node].children.clear();
        if !text.is_empty() {
            let index = self.nodes.len();
            self.nodes.push(Node {
                kind: NodeKind::Text,
                parent: Some(*node),
                children: vec![],
                tag: "",
                text: text.into(),
                attrs: BTreeMap::new(),
            });
            self.nodes[*node].children.push(index);
        }
        Ok(())
    }
    fn attribute(&self, node: &usize, name: &str) -> Result<Option<String>, Error> {
        Ok(self.nodes[*node].attrs.get(name).cloned())
    }
    fn set_attribute(&mut self, node: &usize, name: &str, value: &str) -> Result<(), Error> {
        self.nodes[*node].attrs.insert(name.into(), value.into());
        Ok(())
    }
    fn remove_attribute(&mut self, node: &usize, name: &str) -> Result<(), Error> {
        self.nodes[*node].attrs.remove(name);
        Ok(())
    }
    fn property(&self, node: &usize, name: &str) -> Result<Value, Error> {
        Ok(self
            .properties
            .as_ref()
            .ok_or(Error::Unsupported("property getter"))?
            .get(&(*node, name.into()))
            .cloned()
            .unwrap_or(Value::Undefined))
    }
    fn set_property(&mut self, node: &usize, name: &str, value: Value) -> Result<(), Error> {
        self.properties
            .as_mut()
            .ok_or(Error::Unsupported("property setter"))?
            .insert((*node, name.into()), value);
        Ok(())
    }
    fn remove_property(&mut self, node: &usize, name: &str) -> Result<(), Error> {
        self.properties
            .as_mut()
            .ok_or(Error::Unsupported("property removal"))?
            .remove(&(*node, name.into()));
        Ok(())
    }
    fn clean_data(&mut self, node: &usize) -> Result<(), Error> {
        self.cleaned.push(*node);
        Ok(())
    }
    fn append(&mut self, target: &usize, node: &usize) -> Result<(), Error> {
        self.detach(node)?;
        self.nodes[*target].children.push(*node);
        self.nodes[*node].parent = Some(*target);
        Ok(())
    }
    fn prepend(&mut self, target: &usize, node: &usize) -> Result<(), Error> {
        self.detach(node)?;
        self.nodes[*target].children.insert(0, *node);
        self.nodes[*node].parent = Some(*target);
        Ok(())
    }
    fn insert_after(&mut self, target: &usize, node: &usize) -> Result<(), Error> {
        self.detach(node)?;
        if let Some(p) = self.parent(target) {
            let i = self.nodes[p]
                .children
                .iter()
                .position(|n| n == target)
                .unwrap();
            self.nodes[p].children.insert(i + 1, *node);
            self.nodes[*node].parent = Some(p);
        }
        Ok(())
    }
    fn detach(&mut self, node: &usize) -> Result<(), Error> {
        if let Some(p) = self.parent(node) {
            self.nodes[p].children.retain(|n| n != node);
        }
        self.nodes[*node].parent = None;
        Ok(())
    }
    // This fixture has no template/frame elements; ordinary children are its
    // explicit contents capability, rather than an assumed library fallback.
    fn contents(&self, node: &usize) -> Result<Vec<usize>, Error> {
        Ok(self.children(node))
    }
}
#[test]
fn stacks_traversal_and_ordering() {
    let jq = JQuery::new(Tree::fixture());
    let ps = jq.query("p").unwrap();
    assert_eq!(ps.nodes(), [2, 6, 8]);
    assert_eq!(ps.eq(-2).nodes(), [6]);
    assert_eq!(ps.even().nodes(), [2, 8]);
    assert_eq!(ps.odd().nodes(), [6]);
    assert_eq!(ps.slice(-2, Some(-1)).nodes(), [6]);
    assert_eq!(ps.first().end().nodes(), ps.nodes());
    assert_eq!(ps.end().len(), 0);
    assert_eq!(ps.parents(None).unwrap().nodes(), [5, 1]);
    assert_eq!(ps.closest("main", None).unwrap().nodes(), [1]);
    assert_eq!(ps.eq(1).parents_until("main", None).unwrap().nodes(), [5]);
    assert_eq!(jq.from_nodes([8]).prev_all(None).unwrap().nodes(), [5, 2]);
    assert_eq!(jq.from_nodes([2]).next(None).unwrap().nodes(), [5]);
    assert_eq!(
        jq.from_nodes([2, 8]).siblings(None).unwrap().nodes(),
        [2, 5, 8]
    );
    assert_eq!(
        jq.from_nodes([1]).children(None).unwrap().nodes(),
        [2, 5, 8]
    );
    assert_eq!(jq.from_nodes([1]).contents().unwrap().nodes(), [2, 4, 5, 8]);
    assert_eq!(ps.index(), Some(0));
    assert!(jq.from_nodes([1]).contains(&6));
    assert!(!jq.from_nodes([1]).contains(&1));
    assert_eq!(
        jq.from_nodes([1, 5, 8])
            .has(&jq.from_nodes([6]))
            .unwrap()
            .nodes(),
        [1, 5]
    );
}
#[test]
fn classes_attributes_text_and_host_errors() {
    let jq = JQuery::new(Tree::fixture());
    let ps = jq.query("p").unwrap();
    ps.add_class("a\tb a")
        .unwrap()
        .toggle_class("a c", None)
        .unwrap();
    assert_eq!(ps.attr("class").unwrap().as_deref(), Some("b c"));
    assert!(ps.has_class("c").unwrap());
    ps.remove_class("b").unwrap();
    assert_eq!(ps.attr("class").unwrap().as_deref(), Some("c"));
    ps.set_attr("disabled", &true.into()).unwrap();
    ps.set_attr("disabled", &false.into()).unwrap();
    assert_eq!(ps.attr("disabled").unwrap(), None);
    ps.set_attr("aria-hidden", &false.into()).unwrap();
    assert_eq!(ps.attr("aria-hidden").unwrap().as_deref(), Some("false"));
    assert_eq!(ps.text(), "onetwo");
    ps.first()
        .set_text_with(|i, _, old| format!("{i}:{old}"))
        .unwrap();
    assert_eq!(ps.text(), "0:onetwo");
    assert!(matches!(ps.html(), Err(Error::Unsupported(_))));
    assert!(matches!(ps.prop("value"), Err(Error::Unsupported(_))));
    assert!(matches!(
        ps.add(&JQuery::new(Tree::fixture()).empty()),
        Err(Error::ForeignDocument)
    ));
    assert_eq!(jq.from_nodes([3]).not("p").unwrap().len(), 0);
    ps.remove_all_classes().unwrap();
    assert_eq!(ps.attr("class").unwrap(), Some(String::new()));
}
#[test]
fn prepend_after_keep_batch_order_and_self_insertion_is_ignored() {
    let jq = JQuery::new(Tree::fixture());
    jq.from_nodes([1]).prepend(&jq.from_nodes([8, 5])).unwrap();
    assert_eq!(jq.host().borrow().nodes[1].children, [8, 5, 2, 4]);
    jq.from_nodes([8]).after(&jq.from_nodes([4, 2])).unwrap();
    assert_eq!(jq.host().borrow().nodes[1].children, [8, 4, 2, 5]);
    jq.from_nodes([1]).append(&jq.from_nodes([1])).unwrap();
    assert_eq!(jq.host().borrow().nodes[1].parent, Some(0));
}
#[test]
fn empty_and_remove_route_cleanup_but_detach_retains_data() {
    let jq = JQuery::new(Tree::fixture());
    jq.from_nodes([2]).detach().unwrap();
    assert!(jq.host().borrow().cleaned.is_empty());
    jq.from_nodes([5]).remove().unwrap();
    assert_eq!(jq.host().borrow().cleaned, [5]);
    assert_eq!(jq.query("p").unwrap().nodes(), [8]);
    let _ = Value::Undefined;
}

#[test]
fn document_parents_and_non_element_indices_follow_traversing_source() {
    let jq = JQuery::new(Tree::fixture());
    assert_eq!(jq.from_nodes([1]).parent(None).unwrap().nodes(), [0]);
    assert!(jq.from_nodes([1]).parent(Some("main")).unwrap().is_empty());
    assert!(jq.from_nodes([1]).parents(None).unwrap().is_empty());
    assert_eq!(jq.from_nodes([3]).index(), Some(0));
    assert_eq!(jq.from_nodes([4]).index(), Some(1));
}

#[test]
fn access_and_mutation_skip_unsupported_node_kinds_before_host_calls() {
    let jq = JQuery::new(Tree::fixture());
    // The fixture has no property capability until explicitly enabled.
    jq.from_nodes([2])
        .set_prop("value", Value::Undefined)
        .unwrap();
    jq.from_nodes([3, 4])
        .set_prop("value", true.into())
        .unwrap();
    assert_eq!(jq.from_nodes([3]).prop("value").unwrap(), Value::Undefined);
    let before = jq.host().borrow().nodes[1].children.clone();
    jq.from_nodes([3]).append(&jq.from_nodes([8])).unwrap();
    jq.from_nodes([4]).prepend(&jq.from_nodes([8])).unwrap();
    assert_eq!(jq.host().borrow().nodes[1].children, before);
    jq.from_nodes([3]).remove().unwrap();
    assert!(jq.host().borrow().cleaned.is_empty());
}

#[test]
fn property_name_fixes_apply_to_html_and_preserve_xml_element_names() {
    let mut tree = Tree::fixture();
    tree.properties = Some(BTreeMap::new());
    let jq = JQuery::new(tree);
    let node = jq.from_nodes([2]);
    node.set_prop("class", "native".into()).unwrap();
    node.set_prop("readonly", true.into()).unwrap();
    assert_eq!(node.prop("className").unwrap(), "native".into());
    assert_eq!(node.prop("readOnly").unwrap(), true.into());
    assert_eq!(node.prop("CLASS").unwrap(), Value::Undefined);
    node.remove_prop("class").unwrap();
    assert_eq!(node.prop("className").unwrap(), Value::Undefined);
    jq.host().borrow_mut().xml = true;
    node.set_prop("class", "xml".into()).unwrap();
    assert_eq!(node.prop("class").unwrap(), "xml".into());
    assert_eq!(node.prop("className").unwrap(), Value::Undefined);
    // Upstream removeProp uses propFix unconditionally, including for XML.
    node.remove_prop("class").unwrap();
    assert_eq!(node.prop("class").unwrap(), "xml".into());
}

#[test]
fn positional_dispatch_uses_seed_matching_and_global_is_membership() {
    let jq = JQuery::new(Tree::fixture());
    let seed = jq.from_nodes([2, 2, 6]);
    assert_eq!(seed.filter(":first").unwrap().nodes(), [2]);
    assert_eq!(seed.not(":first").unwrap().nodes(), [2, 6]);
    assert!(jq.from_nodes([2]).is("p:first").unwrap());
    assert!(!jq.from_nodes([6]).is("p:first").unwrap());
    assert!(
        jq.from_nodes([6])
            .closest("p:first", None)
            .unwrap()
            .is_empty()
    );
    assert!(jq.from_nodes([3, 4]).find("*").unwrap().is_empty());
    assert_eq!(jq.from_nodes([3, 4, 2]).not("").unwrap().nodes(), [3, 4, 2]);
    assert!(seed.filter("").unwrap().is_empty());
}

// An ordinary selector host intentionally does not opt into seeded positional
// extensions or frame/template contents. Its defaults must report that gap.
struct OrdinaryHost(Tree);
impl Dom for OrdinaryHost {
    type Node = usize;
    fn query(&self, context: Option<&usize>, selector: &str) -> Result<Vec<usize>, Error> {
        self.0.query(context, selector)
    }
    fn matches(&self, node: &usize, selector: &str) -> Result<bool, Error> {
        self.0.matches(node, selector)
    }
    fn kind(&self, node: &usize) -> NodeKind {
        self.0.kind(node)
    }
    fn parent(&self, node: &usize) -> Option<usize> {
        self.0.parent(node)
    }
    fn children(&self, node: &usize) -> Vec<usize> {
        self.0.children(node)
    }
    fn unique_sort(&self, nodes: &mut Vec<usize>) {
        self.0.unique_sort(nodes);
    }
    fn text(&self, node: &usize) -> String {
        self.0.text(node)
    }
}

#[test]
fn absent_seed_and_special_contents_capabilities_are_explicit() {
    let jq = JQuery::new(OrdinaryHost(Tree::fixture()));
    assert!(matches!(
        jq.from_nodes([2, 6]).filter(":first"),
        Err(Error::Unsupported("seed selector matching"))
    ));
    assert!(matches!(
        jq.from_nodes([2, 6]).not(":first"),
        Err(Error::Unsupported("seed selector matching"))
    ));
    assert!(matches!(
        jq.from_nodes([1]).contents(),
        Err(Error::Unsupported("contents traversal"))
    ));
    assert_eq!(
        jq.from_nodes([2, 6, 5]).filter("p").unwrap().nodes(),
        [2, 6]
    );
    assert_eq!(jq.from_nodes([2, 6, 5]).not("p").unwrap().nodes(), [5]);
}

#[test]
fn toggling_a_duplicate_token_preserves_the_sequential_attribute_updates() {
    let jq = JQuery::new(Tree::fixture());
    let selected = jq.from_nodes([2]);
    assert_eq!(selected.attr("class").unwrap(), None);
    selected.toggle_class("a a", None).unwrap();
    assert_eq!(selected.attr("class").unwrap(), Some(String::new()));
}
