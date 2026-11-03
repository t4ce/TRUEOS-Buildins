//! Native rjqry collection algorithms over the existing bounded DOM gateway.
//! No second tree or guest I/O capability is introduced.
use super::{DomResult, Gateway, failure, input};
use rjqry::{Dom, Error, JQuery, NodeKind};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

fn error(error: Error) -> (&'static str, String) {
    match error {
        Error::Unsupported(_) => failure("NotSupportedError", error.to_string()),
        Error::Host(message) => {
            let (name, detail) = message.split_once(": ").unwrap_or(("Error", &message));
            failure(
                match name {
                    "QuotaExceededError" => "QuotaExceededError",
                    "SyntaxError" => "SyntaxError",
                    "InvalidCharacterError" => "InvalidCharacterError",
                    "TypeError" => "TypeError",
                    _ => "NotSupportedError",
                },
                detail,
            )
        }
        _ => failure("TypeError", error.to_string()),
    }
}
fn host_error((name, message): (&'static str, String)) -> Error {
    Error::Host(format!("{name}: {message}"))
}
struct Adapter<'a> {
    gateway: &'a mut Gateway,
    deadline: Instant,
}
impl Adapter<'_> {
    fn check(&self) -> Result<(), Error> {
        if Instant::now() >= self.deadline {
            Err(Error::Host(
                "QuotaExceededError: native jQuery turn budget exceeded".into(),
            ))
        } else {
            Ok(())
        }
    }
    fn read(&self, request: Value) -> Result<Value, Error> {
        self.check()?;
        // Reads need no mutable accounting; this gateway clone shares the actual
        // arena. Mutations always use the original counters below.
        let mut reader = Gateway {
            style_locks:self.gateway.style_locks.clone(),
            timers:self.gateway.timers.clone(),
            media:self.gateway.media.clone(),
            cookies: self.gateway.cookies.clone(),
            document: self.gateway.document.clone(),
            current_script: self.gateway.current_script.clone(),
            page_url: self.gateway.page_url.clone(),
            window: self.gateway.window.clone(),
            initial_nodes: self.gateway.initial_nodes,
            mutation_bytes: self.gateway.mutation_bytes,
        };
        reader.call(&request).map_err(host_error)
    }
}
impl Dom for Adapter<'_> {
    type Node = usize;
    fn query(&self, context: Option<&usize>, selector: &str) -> Result<Vec<usize>, Error> {
        let root = self.gateway.document.borrow().root;
        let value = self.read(json!(["query", context.copied().unwrap_or(root), selector]))?;
        serde_json::from_value(value).map_err(|e| Error::Host(e.to_string()))
    }
    fn matches(&self, _: &usize, _: &str) -> Result<bool, Error> {
        Err(Error::Unsupported("native jQuery seeded selector matching"))
    }
    fn kind(&self, node: &usize) -> NodeKind {
        if self.check().is_err() {
            return NodeKind::Other;
        }
        match self.gateway.document.borrow().nodes[*node].kind {
            rp5::NodeKind::Document => NodeKind::Document,
            rp5::NodeKind::Fragment => NodeKind::Fragment,
            rp5::NodeKind::Element { .. } => NodeKind::Element,
            rp5::NodeKind::Text(_) => NodeKind::Text,
            rp5::NodeKind::Comment(_) => NodeKind::Comment,
            _ => NodeKind::Other,
        }
    }
    fn parent(&self, node: &usize) -> Option<usize> {
        if self.check().is_err() {
            return None;
        }
        self.gateway.document.borrow().nodes[*node].parent
    }
    fn children(&self, node: &usize) -> Vec<usize> {
        if self.check().is_err() {
            return Vec::new();
        }
        self.gateway.document.borrow().nodes[*node].children.clone()
    }
    fn unique_sort(&self, nodes: &mut Vec<usize>) {
        let doc = self.gateway.document.borrow();
        let mut order = HashMap::new();
        let mut stack = vec![doc.root];
        while let Some(id) = stack.pop() {
            if self.check().is_err() {
                return;
            }
            order.insert(id, order.len());
            stack.extend(doc.nodes[id].children.iter().rev().copied());
        }
        nodes.sort_by_key(|id| order.get(id).copied().unwrap_or(usize::MAX));
        let mut seen = HashSet::new();
        nodes.retain(|id| seen.insert(*id));
    }
    fn text(&self, node: &usize) -> String {
        if self.check().is_err() {
            return String::new();
        }
        self.gateway.document.borrow().text_content(*node)
    }
    fn attribute(&self, node: &usize, name: &str) -> Result<Option<String>, Error> {
        let value = self.read(json!(["attribute", node, "get", name]))?;
        Ok(value.as_str().map(str::to_owned))
    }
    fn set_attribute(&mut self, node: &usize, name: &str, value: &str) -> Result<(), Error> {
        self.check()?;
        self.gateway
            .call(&json!(["attribute", node, "set", name, value]))
            .map(|_| ())
            .map_err(host_error)
    }
    fn remove_attribute(&mut self, node: &usize, name: &str) -> Result<(), Error> {
        self.check()?;
        self.gateway
            .call(&json!(["attribute", node, "remove", name]))
            .map(|_| ())
            .map_err(host_error)
    }
    fn set_text(&mut self, node: &usize, text: &str) -> Result<(), Error> {
        self.check()?;
        self.gateway
            .call(&json!(["set", node, "textContent", text]))
            .map(|_| ())
            .map_err(host_error)
    }
    fn clean_data(&mut self, _: &usize) -> Result<(), Error> {
        // This admission has no per-node jQuery data/event stores to discard.
        Ok(())
    }
}

fn data(value: &Value, depth: usize, nodes: &mut usize) -> DomResult<rjqry::Value> {
    *nodes += 1;
    if depth > 32 || *nodes > 2048 {
        return Err(failure(
            "QuotaExceededError",
            "jQuery data conversion budget exceeded",
        ));
    }
    let parts = value
        .as_array()
        .ok_or_else(|| failure("TypeError", "Expected tagged jQuery data"))?;
    let tag = input(parts, 0)?;
    Ok(match tag {
        "undefined" => rjqry::Value::Undefined,
        "null" => rjqry::Value::Null,
        "bool" => rjqry::Value::Bool(
            parts
                .get(1)
                .and_then(Value::as_bool)
                .ok_or_else(|| failure("TypeError", "Expected boolean"))?,
        ),
        "number" => rjqry::Value::Number(
            input(parts, 1)?
                .parse()
                .map_err(|_| failure("TypeError", "Expected number"))?,
        ),
        "string" => rjqry::Value::String(input(parts, 1)?.into()),
        "array" => rjqry::Value::Array(
            parts
                .iter()
                .skip(1)
                .map(|v| data(v, depth + 1, nodes))
                .collect::<DomResult<_>>()?,
        ),
        "object" => {
            if parts.len() > 129 {
                return Err(failure(
                    "QuotaExceededError",
                    "jQuery object width exceeds native work budget",
                ));
            }
            let mut entries = Vec::new();
            for entry in parts.iter().skip(1) {
                let entry = entry
                    .as_array()
                    .ok_or_else(|| failure("TypeError", "Expected entry"))?;
                let key = input(entry, 0)?.to_owned();
                let value = entry
                    .get(1)
                    .ok_or_else(|| failure("TypeError", "Expected entry value"))?;
                entries.push((key, data(value, depth + 1, nodes)?));
            }
            rjqry::Value::Object(entries)
        }
        _ => return Err(failure("TypeError", "Unsupported jQuery data tag")),
    })
}
pub(super) fn call(gateway: &mut Gateway, args: &[Value]) -> DomResult<Value> {
    let deadline = Instant::now() + Duration::from_millis(20);
    let result = call_inner(gateway, args, deadline);
    if Instant::now() >= deadline {
        return Err(failure(
            "QuotaExceededError",
            "Native jQuery turn budget exceeded",
        ));
    }
    result
}
fn call_inner(gateway: &mut Gateway, args: &[Value], deadline: Instant) -> DomResult<Value> {
    let op = input(args, 1)?;
    if op == "setText" && input(args, 3)?.len() > super::MAX_STRING_BYTES {
        return Err(failure(
            "QuotaExceededError",
            "DOM mutation budget exceeded",
        ));
    }
    if op == "escapeSelector" {
        if input(args, 2)?.len() > super::MAX_STRING_BYTES {
            return Err(failure(
                "QuotaExceededError",
                "jQuery selector string budget exceeded",
            ));
        }
        return Ok(json!(rjqry::utilities::escape_selector(input(args, 2)?)));
    }
    if op == "param" {
        let value = data(
            args.get(2)
                .ok_or_else(|| failure("TypeError", "Expected param data"))?,
            0,
            &mut 0,
        )?;
        let encoded = rjqry::serialize::param(
            &value,
            args.get(3).and_then(Value::as_bool).unwrap_or(false),
        )
        .map_err(|e| failure("NotSupportedError", format!("jQuery.param: {e:?}")))?;
        return Ok(json!(encoded));
    }
    let raw = args
        .get(2)
        .and_then(Value::as_array)
        .ok_or_else(|| failure("TypeError", "Expected node handles"))?;
    let count = gateway.document.borrow().nodes.len();
    if raw.len() > 32768 || count > 100000 {
        return Err(failure(
            "QuotaExceededError",
            "jQuery collection budget exceeded",
        ));
    }
    let ids = raw
        .iter()
        .map(|value| {
            value
                .as_u64()
                .and_then(|id| usize::try_from(id).ok())
                .filter(|id| *id < count)
                .ok_or_else(|| failure("TypeError", "Invalid node handle"))
        })
        .collect::<DomResult<Vec<_>>>()?;
    // The core class algorithm searches an ordered token list. Bound both the
    // incoming and existing lists before entering its quadratic inner loop,
    // which has no host callbacks at which to observe the deadline.
    if matches!(op, "addClass" | "removeClass" | "toggleClass")
        && args.get(3).is_some_and(Value::is_string)
    {
        fn check_classes(value: &str) -> DomResult<()> {
            if value.len() > 16 * 1024
                || value
                    .split(rjqry::utilities::html_space)
                    .filter(|s| !s.is_empty())
                    .take(257)
                    .count()
                    > 256
            {
                return Err(failure(
                    "QuotaExceededError",
                    "jQuery class token work budget exceeded",
                ));
            }
            Ok(())
        }
        check_classes(input(args, 3)?)?;
        let doc = gateway.document.borrow();
        for id in &ids {
            if Instant::now() >= deadline {
                return Err(failure(
                    "QuotaExceededError",
                    "Native jQuery turn budget exceeded",
                ));
            }
            if let Some((_, _, attrs)) = doc.element(*id) {
                if let Some(attr) = attrs.iter().find(|a| super::qualified_name(a) == "class") {
                    check_classes(&attr.value)?;
                }
            }
        }
    }
    let jq = JQuery::new(Adapter { gateway, deadline });
    let selection = jq.from_nodes(ids);
    let text = |index| input(args, index);
    let index = |index| {
        args.get(index)
            .and_then(Value::as_i64)
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| failure("TypeError", "Expected integer index"))
    };
    let result = match op {
        "query" => jq.query(text(3)?).map_err(error)?,
        "find" => selection.find(text(3)?).map_err(error)?,
        "eq" => selection.eq(index(3)?),
        "first" => selection.first(),
        "last" => selection.last(),
        "even" => selection.even(),
        "odd" => selection.odd(),
        "slice" => selection.slice(
            index(3)?,
            if args.get(4).is_none_or(Value::is_null) {
                None
            } else {
                Some(index(4)?)
            },
        ),
        "parent" => selection.parent(None).map_err(error)?,
        "parents" => selection.parents(None).map_err(error)?,
        "children" => selection.children(None).map_err(error)?,
        "siblings" => selection.siblings(None).map_err(error)?,
        "next" => selection.next(None).map_err(error)?,
        "prev" => selection.prev(None).map_err(error)?,
        "nextAll" => selection.next_all(None).map_err(error)?,
        "prevAll" => selection.prev_all(None).map_err(error)?,
        "text" => return Ok(json!(selection.text())),
        "setText" => {
            selection.set_text(text(3)?).map_err(error)?;
            return Ok(Value::Null);
        }
        "attr" => return Ok(json!(selection.attr(text(3)?).map_err(error)?)),
        "setAttr" => {
            let value = data(
                args.get(4)
                    .ok_or_else(|| failure("TypeError", "Expected attribute value"))?,
                0,
                &mut 0,
            )?;
            selection.set_attr(text(3)?, &value).map_err(error)?;
            return Ok(Value::Null);
        }
        "removeAttr" => {
            selection.remove_attr(text(3)?).map_err(error)?;
            return Ok(Value::Null);
        }
        "addClass" => {
            selection.add_class(text(3)?).map_err(error)?;
            return Ok(Value::Null);
        }
        "removeClass" => {
            if args.get(3).is_none_or(Value::is_null) {
                selection.remove_all_classes().map_err(error)?;
            } else {
                selection.remove_class(text(3)?).map_err(error)?;
            }
            return Ok(Value::Null);
        }
        "toggleClass" => {
            selection
                .toggle_class(text(3)?, args.get(4).and_then(Value::as_bool))
                .map_err(error)?;
            return Ok(Value::Null);
        }
        "hasClass" => return Ok(json!(selection.has_class(text(3)?).map_err(error)?)),
        _ => {
            return Err(failure(
                "NotSupportedError",
                "Unsupported native jQuery operation",
            ));
        }
    };
    if result.len() > 32768 {
        return Err(failure(
            "QuotaExceededError",
            "jQuery result exceeds collection budget",
        ));
    }
    Ok(json!(result.to_array()))
}
