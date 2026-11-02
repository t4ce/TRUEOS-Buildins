//! Portable registration and dispatch algorithms translated from jQuery 4.0.0
//! `jquery-main/src/event.js` and `src/event/trigger.js` (MIT).
//!
//! This is the registered-handler layer. A browser host must separately connect
//! native listeners, inline `on*` handlers, the document's window, native default
//! actions, and focus/blur's native state synchronization. No browser event is
//! manufactured here. `trigger` propagates synthetic registered handlers through
//! the actual parent chain supplied by `Dom`; `dispatch` is the native bridge.
//! Event timestamps and pointer/keyboard properties come from the caller.

use crate::{Dom, Error, NodeKind, Value};
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;

type HandlerFn<N> = dyn Fn(&mut Event<N>, &[Value]) -> Value;

/// A cloned handler retains its identity, as an upstream function retains guid.
pub struct Handler<N>(Rc<HandlerFn<N>>);

impl<N> Handler<N> {
    pub fn new(handler: impl Fn(&mut Event<N>, &[Value]) -> Value + 'static) -> Self {
        Self(Rc::new(handler))
    }

    pub fn same(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    pub fn call(&self, event: &mut Event<N>, args: &[Value]) -> Value {
        (self.0)(event, args)
    }
}

impl<N> Clone for Handler<N> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<N> fmt::Debug for Handler<N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Handler").finish_non_exhaustive()
    }
}

/// The binding exposed as `event.handleObj` during dispatch.
#[derive(Clone, Debug)]
pub struct HandleObject<N> {
    pub event_type: String,
    pub original_type: String,
    pub namespace: String,
    pub selector: Option<String>,
    pub data: Value,
    pub handler: Handler<N>,
}

/// Writable native counterpart to jQuery.Event, including propagation flags.
///
/// The host can supply additional keyboard/pointer properties in `properties`;
/// `button` and `related_target` are typed because dispatch uses them directly.
#[derive(Clone, Debug)]
pub struct Event<N> {
    pub event_type: String,
    pub namespace: String,
    pub target: N,
    pub current_target: Option<N>,
    pub delegate_target: Option<N>,
    pub related_target: Option<N>,
    pub button: Option<i16>,
    pub time_stamp: Option<f64>,
    /// `0` for a caller-supplied native event, `2` for trigger_handler, `3` for trigger.
    pub is_trigger: u8,
    pub is_simulated: bool,
    pub properties: Value,
    pub data: Value,
    pub result: Value,
    pub handle_obj: Option<HandleObject<N>>,
    namespaces: Vec<String>,
    default_prevented: bool,
    propagation_stopped: bool,
    immediate_propagation_stopped: bool,
}

impl<N> Event<N> {
    pub fn new(event_type: &str, target: N) -> Result<Self, Error> {
        let name = EventName::parse(event_type);
        if name.event_type.is_empty() {
            return Err(Error::InvalidInput("an event type is required"));
        }
        Ok(Self {
            event_type: name.event_type,
            namespace: name.namespaces.join("."),
            target,
            current_target: None,
            delegate_target: None,
            related_target: None,
            button: None,
            time_stamp: None,
            is_trigger: 0,
            is_simulated: false,
            properties: Value::Object(Vec::new()),
            data: Value::Undefined,
            result: Value::Undefined,
            handle_obj: None,
            namespaces: name.namespaces,
            default_prevented: false,
            propagation_stopped: false,
            immediate_propagation_stopped: false,
        })
    }

    pub fn prevent_default(&mut self) {
        self.default_prevented = true;
    }

    pub fn is_default_prevented(&self) -> bool {
        self.default_prevented
    }

    pub fn stop_propagation(&mut self) {
        self.propagation_stopped = true;
    }

    pub fn is_propagation_stopped(&self) -> bool {
        self.propagation_stopped
    }

    pub fn stop_immediate_propagation(&mut self) {
        self.immediate_propagation_stopped = true;
        self.stop_propagation();
    }

    pub fn is_immediate_propagation_stopped(&self) -> bool {
        self.immediate_propagation_stopped
    }
}

#[derive(Clone)]
struct EventName {
    event_type: String,
    namespaces: Vec<String>,
}

impl EventName {
    fn parse(name: &str) -> Self {
        let mut parts = name.split('.');
        let event_type = parts.next().unwrap_or("").to_string();
        let mut namespaces: Vec<_> = parts.map(ToString::to_string).collect();
        if namespaces.len() == 1 && namespaces[0].is_empty() {
            namespaces.clear();
        }
        namespaces.sort();
        Self {
            event_type,
            namespaces,
        }
    }
}

fn types(types: &str) -> impl DoubleEndedIterator<Item = &str> {
    types
        .split(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{000c}'))
        .filter(|s| !s.is_empty())
}

// With ordinary namespace tokens this is the sorted-subsequence test performed
// by upstream's namespace regexp. Repeated tokens retain their multiplicity.
fn namespace_matches(required: &[String], registered: &[String]) -> bool {
    let mut index = 0;
    for token in registered {
        if required.get(index) == Some(token) {
            index += 1;
        }
    }
    index == required.len()
}

fn mapped_type(original: &str, delegated: bool) -> &str {
    match original {
        "mouseenter" => "mouseover",
        "mouseleave" => "mouseout",
        "pointerenter" => "pointerover",
        "pointerleave" => "pointerout",
        "focus" if delegated => "focusin",
        "blur" if delegated => "focusout",
        _ => original,
    }
}

fn enter_leave(original: &str) -> bool {
    matches!(
        original,
        "mouseenter" | "mouseleave" | "pointerenter" | "pointerleave"
    )
}

#[derive(Clone)]
struct Binding<N> {
    handle: HandleObject<N>,
    namespaces: Vec<String>,
    once: bool,
}

struct Owner<N> {
    node: N,
    bindings: Vec<Binding<N>>,
}

/// One event registry, shared by its cloned handles. Owner keys require only Eq.
/// Reentrant registration/removal is permitted; no registry borrow spans user
/// callbacks or DOM-host calls.
pub struct EventHub<N>(Rc<RefCell<Vec<Owner<N>>>>);

impl<N> Clone for EventHub<N> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<N> Default for EventHub<N> {
    fn default() -> Self {
        Self(Rc::new(RefCell::new(Vec::new())))
    }
}

impl<N: Clone + Eq> EventHub<N> {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn on<D: Dom<Node = N>>(
        &self,
        dom: &D,
        node: &N,
        event_types: &str,
        selector: Option<&str>,
        data: Value,
        handler: Handler<N>,
    ) -> Result<(), Error> {
        self.add(dom, node, event_types, selector, data, handler, false)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn one<D: Dom<Node = N>>(
        &self,
        dom: &D,
        node: &N,
        event_types: &str,
        selector: Option<&str>,
        data: Value,
        handler: Handler<N>,
    ) -> Result<(), Error> {
        self.add(dom, node, event_types, selector, data, handler, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn add<D: Dom<Node = N>>(
        &self,
        dom: &D,
        node: &N,
        event_types: &str,
        selector: Option<&str>,
        data: Value,
        handler: Handler<N>,
        once: bool,
    ) -> Result<(), Error> {
        // data/var/acceptData.js accepts elements, documents and plain objects,
        // but not text, comments or document fragments.
        if !matches!(
            dom.kind(node),
            NodeKind::Element | NodeKind::Document | NodeKind::Other
        ) {
            return Ok(());
        }
        let selector = selector.filter(|selector| !selector.is_empty());
        if let Some(selector) = selector {
            // Validate at attachment time. Query is also the context-aware
            // boundary for delegated needsContext/positional selectors.
            dom.query(Some(node), selector)?;
        }
        for token in types(event_types).rev() {
            let name = EventName::parse(token);
            if name.event_type.is_empty() {
                continue;
            }
            let binding = Binding {
                handle: HandleObject {
                    event_type: mapped_type(&name.event_type, selector.is_some()).to_string(),
                    original_type: name.event_type,
                    namespace: name.namespaces.join("."),
                    selector: selector.map(ToString::to_string),
                    data: data.clone(),
                    handler: handler.clone(),
                },
                namespaces: name.namespaces,
                once,
            };
            let mut owners = self.0.borrow_mut();
            let index = match owners.iter().position(|owner| &owner.node == node) {
                Some(index) => index,
                None => {
                    owners.push(Owner {
                        node: node.clone(),
                        bindings: Vec::new(),
                    });
                    owners.len() - 1
                }
            };
            let bindings = &mut owners[index].bindings;
            if selector.is_some() {
                let position = bindings
                    .iter()
                    .position(|entry| entry.handle.selector.is_none())
                    .unwrap_or(bindings.len());
                bindings.insert(position, binding);
            } else {
                bindings.push(binding);
            }
        }
        Ok(())
    }

    /// Remove registrations. Empty types match all types; `.namespace` matches
    /// all types with that namespace subset. `**` matches only delegated binds.
    /// The return value is the number of bindings removed.
    pub fn off(
        &self,
        node: &N,
        event_types: &str,
        selector: Option<&str>,
        handler: Option<&Handler<N>>,
    ) -> usize {
        let selector = selector.filter(|selector| !selector.is_empty());
        let mut names: Vec<_> = types(event_types).rev().map(EventName::parse).collect();
        if names.is_empty() {
            names.push(EventName::parse(""));
        }
        let mut owners = self.0.borrow_mut();
        let Some(owner) = owners.iter_mut().find(|owner| &owner.node == node) else {
            return 0;
        };
        let before = owner.bindings.len();
        for name in names {
            owner.bindings.retain(|binding| {
                let handle = &binding.handle;
                let same_type = name.event_type.is_empty()
                    || (name.event_type == handle.original_type
                        && mapped_type(&name.event_type, selector.is_some()) == handle.event_type);
                let same_handler = handler.is_none_or(|handler| handler.same(&handle.handler));
                let same_selector = selector.is_none_or(|selector| {
                    handle.selector.as_deref() == Some(selector)
                        || (selector == "**" && handle.selector.is_some())
                });
                !(same_type
                    && same_handler
                    && same_selector
                    && namespace_matches(&name.namespaces, &binding.namespaces))
            });
        }
        let removed = before - owner.bindings.len();
        owners.retain(|owner| !owner.bindings.is_empty());
        removed
    }

    /// The `off(event)` overload, also used by `one` before calling user code.
    pub fn off_event(&self, event: &Event<N>) -> usize {
        let (Some(owner), Some(handle)) = (&event.delegate_target, &event.handle_obj) else {
            return 0;
        };
        let mut name = handle.original_type.clone();
        if !handle.namespace.is_empty() {
            name.push('.');
            name.push_str(&handle.namespace);
        }
        self.off(
            owner,
            &name,
            handle.selector.as_deref(),
            Some(&handle.handler),
        )
    }

    pub fn len(&self, node: &N) -> usize {
        self.0
            .borrow()
            .iter()
            .find(|owner| &owner.node == node)
            .map_or(0, |owner| owner.bindings.len())
    }

    pub fn has_handlers(&self, node: &N) -> bool {
        self.len(node) > 0
    }

    /// Remove all registrations for one owner, for a host's cleanData path.
    pub fn clean(&self, node: &N) -> usize {
        self.off(node, "", None, None)
    }

    /// Dispatch at one listener owner. Handler groups are snapshotted before
    /// invocation: removing an already queued handler does not remove its turn,
    /// and adding one cannot enter this dispatch. A subsequent ancestor's
    /// dispatch observes changes made by handlers lower in the bubble path.
    pub fn dispatch<D: Dom<Node = N>>(
        &self,
        dom: &D,
        owner: &N,
        event: &mut Event<N>,
        args: &[Value],
    ) -> Result<Value, Error> {
        let bindings: Vec<_> = self
            .0
            .borrow()
            .iter()
            .find(|entry| &entry.node == owner)
            .map(|owner| {
                owner
                    .bindings
                    .iter()
                    .filter(|binding| binding.handle.event_type == event.event_type)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        event.delegate_target = Some(owner.clone());
        let mut queue = Vec::<(N, Vec<Binding<N>>)>::new();
        let delegates: Vec<_> = bindings
            .iter()
            .filter(|binding| binding.handle.selector.is_some())
            .cloned()
            .collect();
        let mut selectors = Vec::<(String, Vec<N>)>::new();
        if !delegates.is_empty()
            && !(event.event_type == "click" && event.button.is_some_and(|button| button >= 1))
        {
            let mut current = Some(event.target.clone());
            let mut seen = Vec::new();
            while let Some(node) = current {
                if &node == owner {
                    break;
                }
                if seen.contains(&node) {
                    return Err(Error::InvalidInput("cyclic DOM parent chain"));
                }
                seen.push(node.clone());
                if dom.kind(&node) == NodeKind::Element {
                    let disabled_click = event.event_type == "click"
                        && dom.property(&node, "disabled")? == Value::Bool(true);
                    if !disabled_click {
                        let mut matched = Vec::new();
                        for binding in &delegates {
                            let selector = binding
                                .handle
                                .selector
                                .as_deref()
                                .expect("delegated selector");
                            let index =
                                match selectors.iter().position(|(name, _)| name == selector) {
                                    Some(index) => index,
                                    None => {
                                        selectors.push((
                                            selector.to_string(),
                                            dom.query(Some(owner), selector)?,
                                        ));
                                        selectors.len() - 1
                                    }
                                };
                            if selectors[index].1.contains(&node) {
                                matched.push(binding.clone());
                            }
                        }
                        if !matched.is_empty() {
                            queue.push((node.clone(), matched));
                        }
                    }
                }
                current = dom.parent(&node);
            }
        }
        let direct: Vec<_> = bindings
            .into_iter()
            .filter(|binding| binding.handle.selector.is_none())
            .collect();
        if !direct.is_empty() {
            queue.push((owner.clone(), direct));
        }

        for (current, bindings) in queue {
            if event.is_propagation_stopped() {
                break;
            }
            event.current_target = Some(current.clone());
            for binding in bindings {
                if event.is_immediate_propagation_stopped() {
                    break;
                }
                if !namespace_matches(&event.namespaces, &binding.namespaces) {
                    continue;
                }
                let is_enter_leave = enter_leave(&binding.handle.original_type);
                if is_enter_leave {
                    if let Some(related) = &event.related_target {
                        if related == &current || contains(dom, &current, related)? {
                            continue;
                        }
                    }
                }
                event.data = binding.handle.data.clone();
                event.handle_obj = Some(binding.handle.clone());
                if binding.once {
                    self.off_event(event);
                }
                let saved_type = if is_enter_leave {
                    Some(core::mem::replace(
                        &mut event.event_type,
                        binding.handle.original_type.clone(),
                    ))
                } else {
                    None
                };
                let returned = binding.handle.handler.call(event, args);
                if let Some(saved_type) = saved_type {
                    event.event_type = saved_type;
                }
                if returned != Value::Undefined {
                    if returned == Value::Bool(false) {
                        event.prevent_default();
                        event.stop_propagation();
                    }
                    event.result = returned;
                }
            }
        }
        if event.event_type == "beforeunload" && event.result != Value::Undefined {
            event.prevent_default();
        }
        Ok(event.result.clone())
    }

    /// Propagate this registry's synthetic handlers. Native inline handlers and
    /// default actions require a browser adapter and are not invoked here.
    pub fn trigger<D: Dom<Node = N>>(
        &self,
        dom: &D,
        target: &N,
        event_type: &str,
        args: &[Value],
    ) -> Result<Event<N>, Error> {
        let mut event = Event::new(event_type, target.clone())?;
        self.trigger_with(dom, target, &mut event, args)?;
        Ok(event)
    }

    /// Invoke this registry's handlers on one node, without bubbling/defaults.
    pub fn trigger_handler<D: Dom<Node = N>>(
        &self,
        dom: &D,
        target: &N,
        event_type: &str,
        args: &[Value],
    ) -> Result<Event<N>, Error> {
        let mut event = Event::new(event_type, target.clone())?;
        self.trigger_handler_with(dom, target, &mut event, args)?;
        Ok(event)
    }

    /// Trigger a caller-provided event, preserving its supplied native fields.
    pub fn trigger_with<D: Dom<Node = N>>(
        &self,
        dom: &D,
        target: &N,
        event: &mut Event<N>,
        args: &[Value],
    ) -> Result<Value, Error> {
        self.trigger_event(dom, target, event, args, false)
    }

    pub fn trigger_handler_with<D: Dom<Node = N>>(
        &self,
        dom: &D,
        target: &N,
        event: &mut Event<N>,
        args: &[Value],
    ) -> Result<Value, Error> {
        self.trigger_event(dom, target, event, args, true)
    }

    fn trigger_event<D: Dom<Node = N>>(
        &self,
        dom: &D,
        target: &N,
        event: &mut Event<N>,
        args: &[Value],
        only_handlers: bool,
    ) -> Result<Value, Error> {
        if matches!(dom.kind(target), NodeKind::Text | NodeKind::Comment) {
            return Ok(Value::Undefined);
        }
        let name = EventName::parse(&event.event_type);
        if name.event_type.is_empty() {
            return Err(Error::InvalidInput("an event type is required"));
        }
        if !only_handlers && matches!(name.event_type.as_str(), "focus" | "blur") {
            return Err(Error::Unsupported(
                "native focus/blur trigger synchronization",
            ));
        }
        if event.event_type.contains('.') {
            event.namespaces = name.namespaces;
        } else {
            event.namespaces = if event.namespace.is_empty() {
                Vec::new()
            } else {
                let mut namespaces: Vec<_> = event
                    .namespace
                    .split('.')
                    .map(ToString::to_string)
                    .collect();
                namespaces.sort();
                namespaces
            };
        }
        event.namespace = event.namespaces.join(".");
        event.result = Value::Undefined;
        event.is_trigger = if only_handlers { 2 } else { 3 };
        let original_type = name.event_type;
        // Build the complete path before any user callback can change the DOM.
        let mut path = alloc::vec![target.clone()];
        if !only_handlers && original_type != "load" {
            let mut current = dom.parent(target);
            while let Some(node) = current {
                if path.contains(&node) {
                    return Err(Error::InvalidInput("cyclic DOM parent chain"));
                }
                path.push(node.clone());
                current = dom.parent(&node);
            }
        }
        let result = (|| {
            for (index, owner) in path.iter().enumerate() {
                if event.is_propagation_stopped() {
                    break;
                }
                event.event_type = mapped_type(&original_type, index != 0).to_string();
                if self.has_bound_type(owner, &event.event_type) {
                    self.dispatch(dom, owner, event, args)?;
                }
            }
            Ok(event.result.clone())
        })();
        event.event_type = original_type;
        result
    }

    fn has_bound_type(&self, node: &N, event_type: &str) -> bool {
        self.0
            .borrow()
            .iter()
            .find(|owner| &owner.node == node)
            .is_some_and(|owner| {
                owner
                    .bindings
                    .iter()
                    .any(|binding| binding.handle.event_type == event_type)
            })
    }
}

fn contains<D: Dom>(dom: &D, owner: &D::Node, node: &D::Node) -> Result<bool, Error> {
    let mut current = dom.parent(node);
    let mut seen = Vec::new();
    while let Some(node) = current {
        if &node == owner {
            return Ok(true);
        }
        if seen.contains(&node) {
            return Err(Error::InvalidInput("cyclic DOM parent chain"));
        }
        seen.push(node.clone());
        current = dom.parent(&node);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use core::cell::Cell;

    struct TestDom {
        // document(0) -> outer(1) -> inner(2) -> leaf(4); sibling(3) under outer.
        parents: RefCell<Vec<Option<usize>>>,
        disabled: RefCell<Vec<bool>>,
    }

    impl Default for TestDom {
        fn default() -> Self {
            Self {
                parents: RefCell::new(vec![None, Some(0), Some(1), Some(1), Some(2), Some(2)]),
                disabled: RefCell::new(vec![false; 6]),
            }
        }
    }

    impl Dom for TestDom {
        type Node = usize;
        fn query(&self, context: Option<&usize>, selector: &str) -> Result<Vec<usize>, Error> {
            if selector == "[invalid" {
                return Err(Error::InvalidInput("invalid selector"));
            }
            let mut nodes = Vec::new();
            for node in 1..5 {
                if self.matches(&node, selector)?
                    && context.is_none_or(|owner| contains(self, owner, &node).unwrap())
                {
                    nodes.push(node);
                }
            }
            Ok(nodes)
        }
        fn matches(&self, node: &usize, selector: &str) -> Result<bool, Error> {
            Ok(match selector {
                "*" => self.kind(node) == NodeKind::Element,
                ".inner" => *node == 2 || *node == 4,
                "#leaf" => *node == 4,
                _ => false,
            })
        }
        fn kind(&self, node: &usize) -> NodeKind {
            match node {
                0 => NodeKind::Document,
                5 => NodeKind::Text,
                _ => NodeKind::Element,
            }
        }
        fn parent(&self, node: &usize) -> Option<usize> {
            self.parents.borrow()[*node]
        }
        fn children(&self, node: &usize) -> Vec<usize> {
            self.parents
                .borrow()
                .iter()
                .enumerate()
                .filter_map(|(index, parent)| (*parent == Some(*node)).then_some(index))
                .collect()
        }
        fn unique_sort(&self, nodes: &mut Vec<usize>) {
            nodes.sort();
            nodes.dedup();
        }
        fn text(&self, _: &usize) -> String {
            String::new()
        }
        fn property(&self, node: &usize, name: &str) -> Result<Value, Error> {
            if name == "disabled" {
                Ok(Value::Bool(self.disabled.borrow()[*node]))
            } else {
                Err(Error::Unsupported("test property"))
            }
        }
    }

    fn record(log: &Rc<RefCell<Vec<(String, usize)>>>, label: &str) -> Handler<usize> {
        let log = log.clone();
        let label = label.to_string();
        Handler::new(move |event, _| {
            log.borrow_mut()
                .push((label.clone(), event.current_target.unwrap()));
            Value::Undefined
        })
    }

    #[test]
    fn delegated_groups_follow_target_ancestors_then_direct_handlers() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        hub.on(
            &dom,
            &1,
            "custom",
            None,
            Value::Undefined,
            record(&log, "direct outer"),
        )
        .unwrap();
        hub.on(
            &dom,
            &1,
            "custom",
            Some(".inner"),
            Value::Undefined,
            record(&log, "delegate"),
        )
        .unwrap();
        hub.on(
            &dom,
            &4,
            "custom",
            None,
            Value::Undefined,
            record(&log, "target"),
        )
        .unwrap();
        hub.on(
            &dom,
            &0,
            "custom",
            None,
            Value::Undefined,
            record(&log, "document"),
        )
        .unwrap();
        hub.trigger(&dom, &4, "custom", &[]).unwrap();
        assert_eq!(
            *log.borrow(),
            vec![
                ("target".into(), 4),
                ("delegate".into(), 4),
                ("delegate".into(), 2),
                ("direct outer".into(), 1),
                ("document".into(), 0),
            ]
        );
    }

    #[test]
    fn vendored_multi_namespace_scenario() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        for name in [
            "custom.test",
            "custom.test2",
            "click.test",
            "click.test.abc",
        ] {
            hub.on(&dom, &1, name, None, Value::Undefined, record(&log, name))
                .unwrap();
        }
        hub.trigger(&dom, &1, "click.a.test", &[]).unwrap();
        assert_eq!(hub.off(&1, "click.a.test", None, None), 0);
        for name in ["click.test.abc", "click.abc", "click.test"] {
            hub.trigger(&dom, &1, name, &[]).unwrap();
        }
        assert_eq!(hub.off(&1, "click.abc", None, None), 1);
        hub.trigger(&dom, &1, "click", &[]).unwrap();
        assert_eq!(hub.off(&1, ".test", None, None), 2);
        hub.trigger(&dom, &1, "custom", &[]).unwrap();
        assert_eq!(
            log.borrow()
                .iter()
                .map(|(label, _)| label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "click.test.abc",
                "click.test.abc",
                "click.test",
                "click.test.abc",
                "click.test",
                "custom.test2",
            ]
        );
    }

    #[test]
    fn namespace_only_binding_noop_trailing_namespace_ignored() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let count = Rc::new(Cell::new(0));
        let handler = Handler::new({
            let count = count.clone();
            move |event, _| {
                assert!(event.namespace.is_empty());
                count.set(count.get() + 1);
                Value::Undefined
            }
        });
        hub.on(&dom, &1, ".whoops", None, Value::Undefined, handler.clone())
            .unwrap();
        assert!(!hub.has_handlers(&1));
        hub.on(&dom, &1, "whoops.", None, Value::Undefined, handler)
            .unwrap();
        hub.trigger(&dom, &1, "whoops.", &[]).unwrap();
        assert_eq!(count.get(), 1);
        assert_eq!(hub.off(&1, "whoops.", None, None), 1);
    }

    #[test]
    fn false_prevents_default_and_bubbling_but_other_same_group_handlers_run() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        hub.on(
            &dom,
            &4,
            "custom",
            None,
            Value::Undefined,
            Handler::new(|_, _| Value::Bool(false)),
        )
        .unwrap();
        hub.on(
            &dom,
            &4,
            "custom",
            None,
            Value::Undefined,
            record(&log, "same node"),
        )
        .unwrap();
        hub.on(
            &dom,
            &1,
            "custom",
            None,
            Value::Undefined,
            record(&log, "parent"),
        )
        .unwrap();
        let event = hub.trigger(&dom, &4, "custom", &[]).unwrap();
        assert_eq!(*log.borrow(), vec![("same node".into(), 4)]);
        assert!(event.is_default_prevented());
        assert!(event.is_propagation_stopped());
        assert!(!event.is_immediate_propagation_stopped());
        assert_eq!(event.result, Value::Bool(false));
    }

    #[test]
    fn delegated_stop_propagation_skips_later_ancestor_and_direct_groups() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        hub.on(
            &dom,
            &1,
            "custom",
            Some(".inner"),
            Value::Undefined,
            Handler::new(|event, _| {
                event.stop_propagation();
                Value::Undefined
            }),
        )
        .unwrap();
        hub.on(
            &dom,
            &1,
            "custom",
            Some(".inner"),
            Value::Undefined,
            record(&log, "same group"),
        )
        .unwrap();
        hub.on(
            &dom,
            &1,
            "custom",
            None,
            Value::Undefined,
            record(&log, "direct"),
        )
        .unwrap();
        hub.trigger(&dom, &4, "custom", &[]).unwrap();
        assert_eq!(*log.borrow(), vec![("same group".into(), 4)]);
    }

    #[test]
    fn stop_immediate_prevents_remaining_same_group_and_bubbling() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        hub.on(
            &dom,
            &4,
            "custom",
            None,
            Value::Undefined,
            Handler::new(|event, _| {
                event.stop_immediate_propagation();
                Value::Undefined
            }),
        )
        .unwrap();
        hub.on(
            &dom,
            &4,
            "custom",
            None,
            Value::Undefined,
            record(&log, "same node"),
        )
        .unwrap();
        hub.on(
            &dom,
            &1,
            "custom",
            None,
            Value::Undefined,
            record(&log, "parent"),
        )
        .unwrap();
        let event = hub.trigger(&dom, &4, "custom", &[]).unwrap();
        assert!(log.borrow().is_empty());
        assert!(event.is_immediate_propagation_stopped());
        assert!(!event.is_default_prevented());
    }

    #[test]
    fn one_removes_before_reentrant_trigger_and_preserves_original_identity() {
        let dom = Rc::new(TestDom::default());
        let hub = EventHub::new();
        let count = Rc::new(Cell::new(0));
        let handler = Handler::new({
            let hub = hub.clone();
            let dom = dom.clone();
            let count = count.clone();
            move |_, _| {
                count.set(count.get() + 1);
                assert!(!hub.has_handlers(&4));
                hub.trigger(dom.as_ref(), &4, "custom.ns", &[]).unwrap();
                Value::Undefined
            }
        });
        hub.one(
            dom.as_ref(),
            &4,
            "custom.ns",
            None,
            Value::Undefined,
            handler.clone(),
        )
        .unwrap();
        hub.trigger(dom.as_ref(), &4, "custom.ns", &[]).unwrap();
        hub.trigger(dom.as_ref(), &4, "custom.ns", &[]).unwrap();
        assert_eq!(count.get(), 1);
        hub.one(
            dom.as_ref(),
            &4,
            "custom.ns",
            None,
            Value::Undefined,
            handler.clone(),
        )
        .unwrap();
        assert_eq!(hub.off(&4, "custom.ns", None, Some(&handler)), 1);
    }

    #[test]
    fn reentrant_removal_keeps_snapshot_and_additions_wait_for_next_dispatch() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        let old = record(&log, "old");
        let new = record(&log, "new");
        let first = Handler::new({
            let hub = hub.clone();
            let old = old.clone();
            let new = new.clone();
            move |_, _| {
                hub.off(&4, "custom", None, Some(&old));
                // Registration can be done without a DOM host only through on,
                // so use a fresh equivalent fixture for selector-free binding.
                hub.on(
                    &TestDom::default(),
                    &4,
                    "custom",
                    None,
                    Value::Undefined,
                    new.clone(),
                )
                .unwrap();
                Value::Undefined
            }
        });
        hub.on(&dom, &4, "custom", None, Value::Undefined, first.clone())
            .unwrap();
        hub.on(&dom, &4, "custom", None, Value::Undefined, old)
            .unwrap();
        hub.trigger(&dom, &4, "custom", &[]).unwrap();
        assert_eq!(*log.borrow(), vec![("old".into(), 4)]);
        hub.off(&4, "custom", None, Some(&first));
        hub.trigger(&dom, &4, "custom", &[]).unwrap();
        assert_eq!(*log.borrow(), vec![("old".into(), 4), ("new".into(), 4)]);
    }

    #[test]
    fn trigger_handler_is_single_node_and_keeps_last_defined_result_and_data() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        hub.on(
            &dom,
            &1,
            "custom",
            None,
            Value::Undefined,
            Handler::new(|_, _| panic!("must not bubble")),
        )
        .unwrap();
        hub.on(
            &dom,
            &4,
            "custom.ns",
            None,
            Value::Number(7.0),
            Handler::new(|event, args| {
                assert_eq!(event.target, 4);
                assert_eq!(event.current_target, Some(4));
                assert_eq!(event.delegate_target, Some(4));
                assert_eq!(event.data, Value::Number(7.0));
                assert_eq!(event.is_trigger, 2);
                assert_eq!(args, &[Value::String("extra".into())]);
                assert_eq!(event.handle_obj.as_ref().unwrap().namespace, "ns");
                Value::Number(42.0)
            }),
        )
        .unwrap();
        hub.on(
            &dom,
            &4,
            "custom.ns",
            None,
            Value::Undefined,
            Handler::new(|_, _| Value::Undefined),
        )
        .unwrap();
        let event = hub
            .trigger_handler(&dom, &4, "custom.ns", &[Value::String("extra".into())])
            .unwrap();
        assert_eq!(event.result, Value::Number(42.0));
    }

    #[test]
    fn off_can_target_delegates_namespaces_and_handler_identity() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let a = Handler::new(|_, _| Value::Undefined);
        let b = Handler::new(|_, _| Value::Undefined);
        hub.on(&dom, &1, "custom.a.b", None, Value::Undefined, a.clone())
            .unwrap();
        hub.on(
            &dom,
            &1,
            "custom.a.b",
            Some(".inner"),
            Value::Undefined,
            a.clone(),
        )
        .unwrap();
        hub.on(&dom, &1, "custom.a", Some("*"), Value::Undefined, b.clone())
            .unwrap();
        assert_eq!(hub.off(&1, ".a", Some("**"), Some(&a)), 1);
        assert_eq!(hub.len(&1), 2);
        assert_eq!(hub.off(&1, "custom", Some(".inner"), None), 0);
        assert_eq!(hub.off(&1, ".a.b", None, None), 1);
        assert_eq!(hub.clean(&1), 1);
        assert!(!hub.has_handlers(&1));
    }

    #[test]
    fn delegation_skips_disabled_click_targets_and_nonprimary_buttons() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        hub.on(
            &dom,
            &1,
            "click",
            Some(".inner"),
            Value::Undefined,
            record(&log, "delegate"),
        )
        .unwrap();
        hub.on(
            &dom,
            &1,
            "click",
            None,
            Value::Undefined,
            record(&log, "direct"),
        )
        .unwrap();
        dom.disabled.borrow_mut()[4] = true;
        hub.trigger(&dom, &4, "click", &[]).unwrap();
        assert_eq!(
            *log.borrow(),
            vec![("delegate".into(), 2), ("direct".into(), 1)]
        );
        log.borrow_mut().clear();
        let mut event = Event::new("click", 4).unwrap();
        event.button = Some(1);
        hub.trigger_with(&dom, &4, &mut event, &[]).unwrap();
        assert_eq!(*log.borrow(), vec![("direct".into(), 1)]);
    }

    #[test]
    fn mouseenter_maps_to_over_and_ignores_related_descendants() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let count = Rc::new(Cell::new(0));
        hub.on(
            &dom,
            &2,
            "mouseenter",
            None,
            Value::Undefined,
            Handler::new({
                let count = count.clone();
                move |event, _| {
                    assert_eq!(event.event_type, "mouseenter");
                    count.set(count.get() + 1);
                    Value::Undefined
                }
            }),
        )
        .unwrap();
        let mut event = Event::new("mouseover", 2).unwrap();
        event.related_target = Some(4);
        hub.dispatch(&dom, &2, &mut event, &[]).unwrap();
        assert_eq!(count.get(), 0);
        event.related_target = Some(3);
        hub.dispatch(&dom, &2, &mut event, &[]).unwrap();
        assert_eq!(count.get(), 1);
        assert_eq!(event.event_type, "mouseover");
        hub.trigger(&dom, &2, "mouseenter", &[]).unwrap();
        assert_eq!(count.get(), 2);
        assert_eq!(hub.off(&2, "mouseover", None, None), 0);
        assert_eq!(hub.off(&2, "mouseenter", None, None), 1);
    }

    #[test]
    fn load_does_not_bubble_and_focus_trigger_requires_native_host() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        hub.on(
            &dom,
            &1,
            "load",
            None,
            Value::Undefined,
            Handler::new(|_, _| panic!("load must not bubble")),
        )
        .unwrap();
        hub.trigger(&dom, &4, "load", &[]).unwrap();
        assert!(matches!(
            hub.trigger(&dom, &4, "focus", &[]),
            Err(Error::Unsupported(_))
        ));
        hub.on(
            &dom,
            &4,
            "focus",
            None,
            Value::Undefined,
            Handler::new(|_, _| Value::Number(1.0)),
        )
        .unwrap();
        assert_eq!(
            hub.trigger_handler(&dom, &4, "focus", &[]).unwrap().result,
            Value::Number(1.0)
        );
    }

    #[test]
    fn propagation_path_is_captured_before_handlers_change_parentage() {
        let dom = Rc::new(TestDom::default());
        let hub = EventHub::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        hub.on(
            dom.as_ref(),
            &4,
            "custom",
            None,
            Value::Undefined,
            Handler::new({
                let dom = dom.clone();
                move |_, _| {
                    dom.parents.borrow_mut()[4] = None;
                    Value::Undefined
                }
            }),
        )
        .unwrap();
        hub.on(
            dom.as_ref(),
            &0,
            "custom",
            None,
            Value::Undefined,
            record(&log, "document"),
        )
        .unwrap();
        hub.trigger(dom.as_ref(), &4, "custom", &[]).unwrap();
        assert_eq!(*log.borrow(), vec![("document".into(), 0)]);
    }

    #[test]
    fn invalid_selectors_fail_at_registration_and_text_nodes_do_not_accept_handlers() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let handler = Handler::new(|_, _| Value::Undefined);
        assert!(
            hub.on(
                &dom,
                &1,
                "custom",
                Some("[invalid"),
                Value::Undefined,
                handler.clone()
            )
            .is_err()
        );
        assert_eq!(hub.len(&1), 0);
        hub.on(&dom, &5, "custom", None, Value::Undefined, handler)
            .unwrap();
        assert_eq!(hub.len(&5), 0);
    }

    #[test]
    fn one_delegation_retains_already_queued_matching_ancestor_groups() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        hub.one(
            &dom,
            &1,
            "custom",
            Some(".inner"),
            Value::Undefined,
            record(&log, "one"),
        )
        .unwrap();
        hub.trigger(&dom, &4, "custom", &[]).unwrap();
        // Upstream one() unbinds before calling the handler; it does not remove
        // the binding objects already present in another handlerQueue group.
        assert_eq!(*log.borrow(), vec![("one".into(), 4), ("one".into(), 2)]);
        assert!(!hub.has_handlers(&1));
        hub.trigger(&dom, &4, "custom", &[]).unwrap();
        assert_eq!(log.borrow().len(), 2);
    }

    #[test]
    fn later_owner_dispatch_observes_registration_by_earlier_owner() {
        let dom = Rc::new(TestDom::default());
        let hub = EventHub::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        hub.on(
            dom.as_ref(),
            &4,
            "custom",
            None,
            Value::Undefined,
            Handler::new({
                let hub = hub.clone();
                let dom = dom.clone();
                let log = log.clone();
                move |_, _| {
                    hub.on(
                        dom.as_ref(),
                        &0,
                        "custom",
                        None,
                        Value::Undefined,
                        record(&log, "new parent"),
                    )
                    .unwrap();
                    Value::Undefined
                }
            }),
        )
        .unwrap();
        hub.trigger(dom.as_ref(), &4, "custom", &[]).unwrap();
        assert_eq!(*log.borrow(), vec![("new parent".into(), 0)]);
    }

    #[test]
    fn delegated_focus_uses_host_focusin_and_preserves_mapped_removal_rules() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let count = Rc::new(Cell::new(0));
        hub.on(
            &dom,
            &1,
            "focus.ns",
            Some("#leaf"),
            Value::Undefined,
            Handler::new({
                let count = count.clone();
                move |event, _| {
                    assert_eq!(event.event_type, "focusin");
                    assert_eq!(event.handle_obj.as_ref().unwrap().original_type, "focus");
                    count.set(count.get() + 1);
                    Value::Undefined
                }
            }),
        )
        .unwrap();
        let mut native = Event::new("focusin", 4).unwrap();
        hub.dispatch(&dom, &1, &mut native, &[]).unwrap();
        assert_eq!(count.get(), 1);
        assert_eq!(hub.off(&1, "focus", None, None), 0);
        assert_eq!(hub.off(&1, "focus.ns", Some("**"), None), 1);
    }

    #[test]
    fn multiple_type_tokens_register_in_upstream_reverse_order() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        let namespaces = Rc::new(RefCell::new(Vec::new()));
        hub.on(
            &dom,
            &1,
            "custom.a\tcustom.b",
            None,
            Value::Undefined,
            Handler::new({
                let namespaces = namespaces.clone();
                move |event, _| {
                    namespaces
                        .borrow_mut()
                        .push(event.handle_obj.as_ref().unwrap().namespace.clone());
                    Value::Undefined
                }
            }),
        )
        .unwrap();
        hub.trigger(&dom, &1, "custom", &[]).unwrap();
        assert_eq!(
            *namespaces.borrow(),
            vec![String::from("b"), String::from("a")]
        );
    }

    #[test]
    fn beforeunload_defined_result_prevents_default_without_fabricating_native_fields() {
        let dom = TestDom::default();
        let hub = EventHub::new();
        hub.on(
            &dom,
            &1,
            "beforeunload",
            None,
            Value::Undefined,
            Handler::new(|event, _| {
                assert_eq!(event.time_stamp, None);
                Value::String("leave?".into())
            }),
        )
        .unwrap();
        let event = hub.trigger_handler(&dom, &1, "beforeunload", &[]).unwrap();
        assert!(event.is_default_prevented());
        assert_eq!(event.result, Value::String("leave?".into()));
    }
}
