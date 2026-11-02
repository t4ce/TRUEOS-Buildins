//! Collection/stack/traversal and attribute algorithms translated from core.js,
//! traversing.js, traversing/findFilter.js, attributes/* and manipulation.js.
use crate::{
    Dom, Error, NodeKind, Value,
    utilities::{html_space, needs_context, prop_fix, strip_and_collapse},
};
use alloc::{rc::Rc, string::String, vec::Vec};
use core::cell::RefCell;

pub struct JQuery<D: Dom> {
    host: Rc<RefCell<D>>,
}
impl<D: Dom> Clone for JQuery<D> {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
        }
    }
}
impl<D: Dom> JQuery<D> {
    pub fn new(host: D) -> Self {
        Self {
            host: Rc::new(RefCell::new(host)),
        }
    }
    pub fn host(&self) -> Rc<RefCell<D>> {
        self.host.clone()
    }
    pub fn from_nodes(&self, nodes: impl IntoIterator<Item = D::Node>) -> Selection<D> {
        Selection {
            host: self.host.clone(),
            nodes: nodes.into_iter().collect(),
            previous: None,
        }
    }
    pub fn empty(&self) -> Selection<D> {
        self.from_nodes(core::iter::empty())
    }
    pub fn query(&self, selector: &str) -> Result<Selection<D>, Error> {
        let nodes = self.host.borrow().query(None, selector)?;
        Ok(self.from_nodes(nodes))
    }
    pub fn query_in(&self, selector: &str, context: &D::Node) -> Result<Selection<D>, Error> {
        let nodes = self.host.borrow().query(Some(context), selector)?;
        Ok(self.from_nodes(nodes))
    }
}
pub struct Selection<D: Dom> {
    host: Rc<RefCell<D>>,
    nodes: Vec<D::Node>,
    previous: Option<Rc<Self>>,
}
impl<D: Dom> Clone for Selection<D> {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            nodes: self.nodes.clone(),
            previous: self.previous.clone(),
        }
    }
}
impl<D: Dom> Selection<D> {
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    pub fn to_array(&self) -> Vec<D::Node> {
        self.nodes.clone()
    }
    pub fn nodes(&self) -> &[D::Node] {
        &self.nodes
    }
    pub fn get(&self, index: isize) -> Option<&D::Node> {
        let index = if index < 0 {
            self.len().checked_sub(index.unsigned_abs())?
        } else {
            index as usize
        };
        self.nodes.get(index)
    }
    pub fn push_stack(&self, nodes: impl IntoIterator<Item = D::Node>) -> Self {
        Self {
            host: self.host.clone(),
            nodes: nodes.into_iter().collect(),
            previous: Some(Rc::new(self.clone())),
        }
    }
    pub fn end(&self) -> Self {
        self.previous.as_ref().map_or_else(
            || Self {
                host: self.host.clone(),
                nodes: Vec::new(),
                previous: None,
            },
            |previous| (**previous).clone(),
        )
    }
    pub fn eq(&self, index: isize) -> Self {
        self.push_stack(self.get(index).cloned())
    }
    pub fn first(&self) -> Self {
        self.eq(0)
    }
    pub fn last(&self) -> Self {
        self.eq(-1)
    }
    pub fn even(&self) -> Self {
        self.push_stack(
            self.nodes
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == 0)
                .map(|(_, node)| node.clone()),
        )
    }
    pub fn odd(&self) -> Self {
        self.push_stack(
            self.nodes
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 2 != 0)
                .map(|(_, node)| node.clone()),
        )
    }
    pub fn slice(&self, start: isize, end: Option<isize>) -> Self {
        let bound = |index: isize| {
            if index < 0 {
                self.len().saturating_sub(index.unsigned_abs())
            } else {
                (index as usize).min(self.len())
            }
        };
        let start = bound(start);
        let end = end.map_or(self.len(), bound).max(start);
        self.push_stack(self.nodes[start..end].iter().cloned())
    }
    pub fn each(&self, mut callback: impl FnMut(usize, &D::Node) -> bool) -> &Self {
        for (i, node) in self.nodes.iter().enumerate() {
            if !callback(i, node) {
                break;
            }
        }
        self
    }
    pub fn map<I: IntoIterator<Item = D::Node>>(
        &self,
        mut callback: impl FnMut(usize, &D::Node) -> I,
    ) -> Self {
        let mut nodes = Vec::new();
        for (i, node) in self.nodes.iter().enumerate() {
            nodes.extend(callback(i, node));
        }
        self.push_stack(nodes)
    }
    fn same_host(&self, other: &Self) -> Result<(), Error> {
        if Rc::ptr_eq(&self.host, &other.host) {
            Ok(())
        } else {
            Err(Error::ForeignDocument)
        }
    }
    pub fn find(&self, selector: &str) -> Result<Self, Error> {
        let host = self.host.borrow();
        let mut nodes = Vec::new();
        for node in &self.nodes {
            if matches!(
                host.kind(node),
                NodeKind::Element | NodeKind::Document | NodeKind::Fragment
            ) {
                nodes.extend(host.query(Some(node), selector)?);
            }
        }
        if self.len() > 1 {
            host.unique_sort(&mut nodes);
        }
        Ok(self.push_stack(nodes))
    }
    pub fn filter(&self, selector: &str) -> Result<Self, Error> {
        self.winnow(selector, false)
    }
    pub fn not(&self, selector: &str) -> Result<Self, Error> {
        self.winnow(selector, true)
    }
    fn winnow(&self, selector: &str, invert: bool) -> Result<Self, Error> {
        if selector.is_empty() {
            return Ok(self.push_stack(if invert { self.to_array() } else { Vec::new() }));
        }
        let host = self.host.borrow();
        let elements = self
            .nodes
            .iter()
            .filter(|node| host.kind(node) == NodeKind::Element)
            .cloned()
            .collect::<Vec<_>>();
        // Negation is part of the seeded expression, not identity subtraction:
        // duplicate seed occurrences can have distinct positional matches.
        let expression = if invert {
            alloc::format!(":not({selector})")
        } else {
            selector.into()
        };
        let nodes = host.filter(&elements, &expression)?;
        Ok(self.push_stack(nodes))
    }
    pub fn filter_with(&self, mut predicate: impl FnMut(usize, &D::Node) -> bool) -> Self {
        self.push_stack(
            self.nodes
                .iter()
                .enumerate()
                .filter(|(i, node)| predicate(*i, node))
                .map(|(_, node)| node.clone()),
        )
    }
    pub fn is(&self, selector: &str) -> Result<bool, Error> {
        if needs_context(selector) {
            let candidates = self.host.borrow().query(None, selector)?;
            Ok(self.nodes.iter().any(|node| candidates.contains(node)))
        } else {
            Ok(!self.filter(selector)?.is_empty())
        }
    }
    pub fn add(&self, other: &Self) -> Result<Self, Error> {
        self.same_host(other)?;
        let mut nodes = self.to_array();
        nodes.extend(other.to_array());
        self.host.borrow().unique_sort(&mut nodes);
        Ok(self.push_stack(nodes))
    }
    pub fn add_back(&self, selector: Option<&str>) -> Result<Self, Error> {
        let previous = self.end();
        self.add(&if let Some(selector) = selector {
            previous.filter(selector)?
        } else {
            previous
        })
    }
    pub fn unique_sort(&self) -> Self {
        let mut nodes = self.to_array();
        self.host.borrow().unique_sort(&mut nodes);
        self.push_stack(nodes)
    }
    pub fn parent(&self, selector: Option<&str>) -> Result<Self, Error> {
        self.traverse(Direction::Parent, false, None, selector)
    }
    pub fn parents(&self, selector: Option<&str>) -> Result<Self, Error> {
        self.traverse(Direction::Parent, true, None, selector)
    }
    pub fn parents_until(&self, until: &str, selector: Option<&str>) -> Result<Self, Error> {
        self.traverse(Direction::Parent, true, Some(until), selector)
    }
    pub fn next(&self, selector: Option<&str>) -> Result<Self, Error> {
        self.traverse(Direction::Next, false, None, selector)
    }
    pub fn prev(&self, selector: Option<&str>) -> Result<Self, Error> {
        self.traverse(Direction::Previous, false, None, selector)
    }
    pub fn next_all(&self, selector: Option<&str>) -> Result<Self, Error> {
        self.traverse(Direction::Next, true, None, selector)
    }
    pub fn prev_all(&self, selector: Option<&str>) -> Result<Self, Error> {
        self.traverse(Direction::Previous, true, None, selector)
    }
    pub fn next_until(&self, until: &str, selector: Option<&str>) -> Result<Self, Error> {
        self.traverse(Direction::Next, true, Some(until), selector)
    }
    pub fn prev_until(&self, until: &str, selector: Option<&str>) -> Result<Self, Error> {
        self.traverse(Direction::Previous, true, Some(until), selector)
    }
    fn traverse(
        &self,
        direction: Direction,
        all: bool,
        until: Option<&str>,
        selector: Option<&str>,
    ) -> Result<Self, Error> {
        let host = self.host.borrow();
        let mut nodes = Vec::new();
        for start in &self.nodes {
            let mut node = start.clone();
            while let Some(next) = step(&*host, &node, direction) {
                node = next;
                if host.kind(&node) == NodeKind::Document {
                    if !all && matches!(direction, Direction::Parent) {
                        nodes.push(node);
                    }
                    break;
                }
                if host.kind(&node) != NodeKind::Element {
                    if !all && matches!(direction, Direction::Parent) {
                        break;
                    }
                    continue;
                }
                if let Some(until) = until {
                    if host.matches(&node, until)? {
                        break;
                    }
                }
                nodes.push(node.clone());
                if !all {
                    break;
                }
            }
        }
        if let Some(selector) = selector {
            let mut filtered = Vec::new();
            for node in nodes {
                if host.kind(&node) == NodeKind::Element && host.matches(&node, selector)? {
                    filtered.push(node);
                }
            }
            nodes = filtered;
        }
        if self.len() > 1 {
            if all || matches!(direction, Direction::Parent) {
                host.unique_sort(&mut nodes);
            }
            if all && matches!(direction, Direction::Parent | Direction::Previous) {
                nodes.reverse();
            }
        }
        Ok(self.push_stack(nodes))
    }
    pub fn children(&self, selector: Option<&str>) -> Result<Self, Error> {
        self.sibling_set(false, selector)
    }
    pub fn siblings(&self, selector: Option<&str>) -> Result<Self, Error> {
        self.sibling_set(true, selector)
    }
    fn sibling_set(&self, siblings: bool, selector: Option<&str>) -> Result<Self, Error> {
        let host = self.host.borrow();
        let mut nodes = Vec::new();
        for node in &self.nodes {
            let children = if siblings {
                host.parent(node)
                    .map_or_else(Vec::new, |p| host.children(&p))
            } else {
                host.children(node)
            };
            for child in children {
                if host.kind(&child) == NodeKind::Element
                    && (!siblings || child != *node)
                    && selector.map_or(Ok(true), |s| host.matches(&child, s))?
                {
                    nodes.push(child);
                }
            }
        }
        if siblings && self.len() > 1 {
            host.unique_sort(&mut nodes);
        }
        Ok(self.push_stack(nodes))
    }
    pub fn contents(&self) -> Result<Self, Error> {
        let host = self.host.borrow();
        let mut nodes = Vec::new();
        for node in &self.nodes {
            nodes.extend(host.contents(node)?);
        }
        Ok(self.push_stack(nodes))
    }
    pub fn closest(&self, selector: &str, context: Option<&D::Node>) -> Result<Self, Error> {
        if needs_context(selector) {
            return Ok(self.push_stack(core::iter::empty()));
        }
        let host = self.host.borrow();
        let mut nodes = Vec::new();
        for start in &self.nodes {
            let mut current = Some(start.clone());
            while let Some(node) = current {
                if context == Some(&node) {
                    break;
                }
                if host.kind(&node) == NodeKind::Element && host.matches(&node, selector)? {
                    nodes.push(node);
                    break;
                }
                current = host.parent(&node);
            }
        }
        if nodes.len() > 1 {
            host.unique_sort(&mut nodes);
        }
        Ok(self.push_stack(nodes))
    }
    pub fn contains(&self, node: &D::Node) -> bool {
        let host = self.host.borrow();
        self.nodes.iter().any(|ancestor| {
            let mut current = host.parent(node);
            while let Some(node) = current {
                if node == *ancestor {
                    return true;
                }
                current = host.parent(&node);
            }
            false
        })
    }
    pub fn has(&self, target: &Self) -> Result<Self, Error> {
        self.same_host(target)?;
        Ok(self.filter_with(|_, node| self.push_stack([node.clone()]).contains_any(&target.nodes)))
    }
    fn contains_any(&self, nodes: &[D::Node]) -> bool {
        nodes.iter().any(|node| self.contains(node))
    }
    pub fn index(&self) -> Option<usize> {
        let node = self.nodes.first()?;
        let host = self.host.borrow();
        let parent = host.parent(node)?;
        let children = host.children(&parent);
        let position = children.iter().position(|n| n == node)?;
        Some(
            children[..position]
                .iter()
                .filter(|n| host.kind(n) == NodeKind::Element)
                .count(),
        )
    }
    pub fn index_of(&self, node: &D::Node) -> Option<usize> {
        self.nodes.iter().position(|n| n == node)
    }
    pub fn text(&self) -> String {
        let host = self.host.borrow();
        self.nodes
            .iter()
            .filter(|n| !matches!(host.kind(n), NodeKind::Comment | NodeKind::Other))
            .map(|node| host.text(node))
            .collect::<Vec<_>>()
            .join("")
    }
    pub fn set_text(&self, text: &str) -> Result<&Self, Error> {
        self.empty()?;
        for node in &self.nodes {
            if matches!(
                self.host.borrow().kind(node),
                NodeKind::Element | NodeKind::Document | NodeKind::Fragment
            ) {
                self.host.borrow_mut().set_text(node, text)?;
            }
        }
        Ok(self)
    }
    pub fn set_text_with(
        &self,
        mut callback: impl FnMut(usize, &D::Node, &str) -> String,
    ) -> Result<&Self, Error> {
        for (i, node) in self.nodes.iter().enumerate() {
            let current = self.host.borrow().text(node);
            let next = callback(i, node, &current);
            self.push_stack([node.clone()]).set_text(&next)?;
        }
        Ok(self)
    }
    pub fn attr(&self, name: &str) -> Result<Option<String>, Error> {
        self.nodes.first().map_or(Ok(None), |node| {
            if matches!(
                self.host.borrow().kind(node),
                NodeKind::Text | NodeKind::Comment | NodeKind::Other
            ) {
                Ok(None)
            } else {
                self.host.borrow().attribute(node, name)
            }
        })
    }
    pub fn set_attr(&self, name: &str, value: &Value) -> Result<&Self, Error> {
        if *value == Value::Undefined {
            return Ok(self);
        }
        for node in &self.nodes {
            if self.host.borrow().kind(node) != NodeKind::Element {
                continue;
            }
            if *value == Value::Null
                || (*value == Value::Bool(false) && !name.to_ascii_lowercase().starts_with("aria-"))
            {
                self.host.borrow_mut().remove_attribute(node, name)?;
            } else {
                self.host
                    .borrow_mut()
                    .set_attribute(node, name, &value.js_string())?;
            }
        }
        Ok(self)
    }
    pub fn remove_attr(&self, names: &str) -> Result<&Self, Error> {
        let names = names
            .split(html_space)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        for node in &self.nodes {
            if self.host.borrow().kind(node) == NodeKind::Element {
                for name in &names {
                    self.host.borrow_mut().remove_attribute(node, name)?;
                }
            }
        }
        Ok(self)
    }
    pub fn prop(&self, name: &str) -> Result<Value, Error> {
        self.nodes.first().map_or(Ok(Value::Undefined), |node| {
            let host = self.host.borrow();
            if matches!(
                host.kind(node),
                NodeKind::Text | NodeKind::Comment | NodeKind::Other
            ) {
                Ok(Value::Undefined)
            } else {
                let name = if host.kind(node) == NodeKind::Element && host.is_xml(node) {
                    name
                } else {
                    prop_fix(name)
                };
                host.property(node, name)
            }
        })
    }
    pub fn set_prop(&self, name: &str, value: Value) -> Result<&Self, Error> {
        if value == Value::Undefined {
            return Ok(self);
        }
        for node in &self.nodes {
            let host = self.host.borrow();
            if matches!(
                host.kind(node),
                NodeKind::Text | NodeKind::Comment | NodeKind::Other
            ) {
                continue;
            }
            let name = if host.kind(node) == NodeKind::Element && host.is_xml(node) {
                name
            } else {
                prop_fix(name)
            };
            drop(host);
            self.host
                .borrow_mut()
                .set_property(node, name, value.clone())?;
        }
        Ok(self)
    }
    pub fn remove_prop(&self, name: &str) -> Result<&Self, Error> {
        for node in &self.nodes {
            self.host
                .borrow_mut()
                .remove_property(node, prop_fix(name))?;
        }
        Ok(self)
    }
    pub fn html(&self) -> Result<Option<String>, Error> {
        self.nodes.first().map_or(Ok(None), |node| {
            if self.host.borrow().kind(node) == NodeKind::Element {
                self.host.borrow().html(node).map(Some)
            } else {
                Ok(None)
            }
        })
    }
    pub fn set_html(&self, html: &str) -> Result<&Self, Error> {
        for node in &self.nodes {
            self.host.borrow_mut().set_html(node, html)?;
        }
        Ok(self)
    }
    pub fn add_class(&self, classes: &str) -> Result<&Self, Error> {
        self.classes(classes, ClassAction::Add)
    }
    pub fn remove_class(&self, classes: &str) -> Result<&Self, Error> {
        self.classes(classes, ClassAction::Remove)
    }
    pub fn remove_all_classes(&self) -> Result<&Self, Error> {
        self.set_attr("class", &Value::String(String::new()))
    }
    pub fn toggle_class(&self, classes: &str, state: Option<bool>) -> Result<&Self, Error> {
        self.classes(
            classes,
            match state {
                Some(true) => ClassAction::Add,
                Some(false) => ClassAction::Remove,
                None => ClassAction::Toggle,
            },
        )
    }
    fn classes(&self, classes: &str, action: ClassAction) -> Result<&Self, Error> {
        let names = classes
            .split(html_space)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if names.is_empty() {
            return Ok(self);
        }
        if matches!(action, ClassAction::Toggle) {
            for node in &self.nodes {
                let one = self.push_stack([node.clone()]);
                for name in &names {
                    if one.has_class(name)? {
                        one.remove_class(name)?;
                    } else {
                        one.add_class(name)?;
                    }
                }
            }
            return Ok(self);
        }
        for node in &self.nodes {
            if self.host.borrow().kind(node) != NodeKind::Element {
                continue;
            }
            let old = self
                .host
                .borrow()
                .attribute(node, "class")?
                .unwrap_or_default();
            let normalized = strip_and_collapse(&old);
            let mut current = normalized
                .split(' ')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect::<Vec<_>>();
            for name in &names {
                let present = current.iter().any(|value| value == name);
                let remove = matches!(action, ClassAction::Remove)
                    || (matches!(action, ClassAction::Toggle) && present);
                if remove {
                    current.retain(|value| value != name);
                } else if !present {
                    current.push((*name).into());
                }
            }
            let next = current.join(" ");
            if next != old {
                self.host.borrow_mut().set_attribute(node, "class", &next)?;
            }
        }
        Ok(self)
    }
    pub fn has_class(&self, class: &str) -> Result<bool, Error> {
        let needle = alloc::format!(" {class} ");
        for node in &self.nodes {
            if self.host.borrow().kind(node) == NodeKind::Element {
                let text = self
                    .host
                    .borrow()
                    .attribute(node, "class")?
                    .unwrap_or_default();
                if alloc::format!(" {} ", strip_and_collapse(&text)).contains(&needle) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    pub fn empty(&self) -> Result<&Self, Error> {
        for node in &self.nodes {
            if self.host.borrow().kind(node) == NodeKind::Element {
                let children = self.host.borrow().children(node);
                for child in children {
                    if self.host.borrow().kind(&child) == NodeKind::Element {
                        self.host.borrow_mut().clean_data(&child)?;
                    }
                }
                self.host.borrow_mut().set_text(node, "")?;
            }
        }
        Ok(self)
    }
    pub fn detach(&self) -> Result<&Self, Error> {
        for node in &self.nodes {
            self.host.borrow_mut().detach(node)?;
        }
        Ok(self)
    }
    pub fn remove(&self) -> Result<&Self, Error> {
        for node in &self.nodes {
            if self.host.borrow().kind(node) == NodeKind::Element {
                self.host.borrow_mut().clean_data(node)?;
            }
            self.host.borrow_mut().detach(node)?;
        }
        Ok(self)
    }
    pub fn clone_nodes(
        &self,
        data_and_events: bool,
        deep_data_and_events: bool,
    ) -> Result<Self, Error> {
        let mut nodes = Vec::new();
        for node in &self.nodes {
            nodes.push(self.host.borrow_mut().clone_node(
                node,
                data_and_events,
                deep_data_and_events,
            )?);
        }
        Ok(self.push_stack(nodes))
    }
    pub fn append(&self, content: &Self) -> Result<&Self, Error> {
        self.insert(content, Insert::Append)
    }
    pub fn prepend(&self, content: &Self) -> Result<&Self, Error> {
        self.insert(content, Insert::Prepend)
    }
    pub fn before(&self, content: &Self) -> Result<&Self, Error> {
        self.insert(content, Insert::Before)
    }
    pub fn after(&self, content: &Self) -> Result<&Self, Error> {
        self.insert(content, Insert::After)
    }
    fn insert(&self, content: &Self, operation: Insert) -> Result<&Self, Error> {
        self.same_host(content)?;
        for (i, target) in self.nodes.iter().enumerate() {
            if matches!(operation, Insert::Append | Insert::Prepend)
                && !matches!(
                    self.host.borrow().kind(target),
                    NodeKind::Element | NodeKind::Document | NodeKind::Fragment
                )
            {
                continue;
            }
            let mut originals = content
                .nodes
                .iter()
                .filter(|node| !self.nodes.contains(node))
                .collect::<Vec<_>>();
            if matches!(operation, Insert::Prepend | Insert::After) {
                originals.reverse();
            }
            for original in originals {
                let node = if i + 1 == self.len() {
                    original.clone()
                } else {
                    self.host.borrow_mut().clone_node(original, true, true)?
                };
                match operation {
                    Insert::Append => self.host.borrow_mut().append(target, &node)?,
                    Insert::Prepend => self.host.borrow_mut().prepend(target, &node)?,
                    Insert::Before => self.host.borrow_mut().insert_before(target, &node)?,
                    Insert::After => self.host.borrow_mut().insert_after(target, &node)?,
                }
            }
        }
        Ok(self)
    }
}
#[derive(Clone, Copy)]
enum Direction {
    Parent,
    Next,
    Previous,
}
#[derive(Clone, Copy)]
enum ClassAction {
    Add,
    Remove,
    Toggle,
}
#[derive(Clone, Copy)]
enum Insert {
    Append,
    Prepend,
    Before,
    After,
}
fn step<D: Dom>(host: &D, node: &D::Node, direction: Direction) -> Option<D::Node> {
    let parent = host.parent(node)?;
    if matches!(direction, Direction::Parent) {
        return Some(parent);
    }
    let children = host.children(&parent);
    let index = children.iter().position(|n| n == node)?;
    children
        .get(match direction {
            Direction::Next => index.checked_add(1)?,
            Direction::Previous => index.checked_sub(1)?,
            Direction::Parent => unreachable!(),
        })
        .cloned()
}
