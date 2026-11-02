//! Original event.js/trigger.js produce the recorded traces through
//! tools/event_oracle.mjs. The Rust side uses the same declarative operations,
//! and parses fixtures with the crate's native reader, without dependencies.
use rjqry::{
    Dom, Error, NodeKind, Value,
    events::{Event, EventHub, Handler},
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}

fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .object_get(key)
        .unwrap_or_else(|| panic!("missing {key} in {value:?}"))
}

fn string(value: &Value) -> &str {
    let Value::String(text) = value else {
        panic!("expected string, got {value:?}")
    };
    text
}

fn array(value: &Value) -> &[Value] {
    let Value::Array(values) = value else {
        panic!("expected array, got {value:?}")
    };
    values
}

fn optional<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.object_get(key)
}

fn bool_field(value: &Value, key: &str) -> bool {
    optional(value, key) == Some(&Value::Bool(true))
}

fn encode(value: Value) -> Value {
    match value {
        Value::Undefined => object([("$undefined", true.into())]),
        Value::Array(values) => Value::Array(values.into_iter().map(encode).collect()),
        Value::Object(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key, encode(value)))
                .collect(),
        ),
        value => value,
    }
}

fn equivalent(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter().all(|(key, value)| {
                    right
                        .object_get(key)
                        .is_some_and(|other| equivalent(value, other))
                })
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equivalent(a, b))
        }
        _ => left == right,
    }
}

/// Just a plain owner object. No DOM, selectors, native listener or default
/// action is simulated, matching the original-source JavaScript fixture.
struct PlainOwner;

impl Dom for PlainOwner {
    type Node = usize;
    fn query(&self, _: Option<&usize>, _: &str) -> Result<Vec<usize>, Error> {
        Err(Error::Unsupported("plain-owner fixture selectors"))
    }
    fn matches(&self, _: &usize, _: &str) -> Result<bool, Error> {
        Err(Error::Unsupported("plain-owner fixture selectors"))
    }
    fn kind(&self, _: &usize) -> NodeKind {
        NodeKind::Other
    }
    fn parent(&self, _: &usize) -> Option<usize> {
        None
    }
    fn children(&self, _: &usize) -> Vec<usize> {
        Vec::new()
    }
    fn unique_sort(&self, _: &mut Vec<usize>) {}
    fn text(&self, _: &usize) -> String {
        String::new()
    }
}

struct Runner {
    hub: EventHub<usize>,
    handlers: RefCell<BTreeMap<String, Handler<usize>>>,
    counts: RefCell<BTreeMap<String, usize>>,
    trace: RefCell<Vec<Value>>,
}

impl Runner {
    fn new(input: &Value) -> Rc<Self> {
        let runner = Rc::new(Self {
            hub: EventHub::new(),
            handlers: RefCell::new(BTreeMap::new()),
            counts: RefCell::new(BTreeMap::new()),
            trace: RefCell::new(Vec::new()),
        });
        for descriptor in array(field(input, "handlers")) {
            let weak = Rc::downgrade(&runner);
            let descriptor = descriptor.clone();
            let id = string(field(&descriptor, "id")).to_string();
            let handler = Handler::new(move |event, args| {
                weak.upgrade()
                    .expect("active oracle runner")
                    .invoke(&descriptor, event, args)
            });
            runner.handlers.borrow_mut().insert(id, handler);
        }
        runner
    }

    fn handler(&self, id: &str) -> Handler<usize> {
        self.handlers
            .borrow()
            .get(id)
            .unwrap_or_else(|| panic!("unknown handler {id}"))
            .clone()
    }

    fn invoke(&self, descriptor: &Value, event: &mut Event<usize>, args: &[Value]) -> Value {
        let id = string(field(descriptor, "id"));
        let count = {
            let mut counts = self.counts.borrow_mut();
            let count = counts.entry(id.to_string()).or_default();
            *count += 1;
            *count
        };
        let mut entry = vec![
            ("kind", "handler".into()),
            ("id", id.into()),
            ("type", event.event_type.clone().into()),
            ("namespace", event.namespace.clone().into()),
            (
                "bindingNamespace",
                event.handle_obj.as_ref().unwrap().namespace.clone().into(),
            ),
            ("data", encode(event.data.clone())),
            ("args", encode(Value::Array(args.to_vec()))),
            ("isTrigger", Value::Number(event.is_trigger as f64)),
        ];
        entry.extend(flags(event));
        self.trace.borrow_mut().push(object(entry));
        match optional(descriptor, "control").map(string) {
            Some("prevent") => event.prevent_default(),
            Some("propagation") => event.stop_propagation(),
            Some("immediate") => event.stop_immediate_propagation(),
            None => (),
            control => panic!("unknown event control {control:?}"),
        }
        if bool_field(descriptor, "offEvent") {
            self.hub.off_event(event);
        }
        if !bool_field(descriptor, "actionsFirstOnly") || count == 1 {
            if let Some(actions) = optional(descriptor, "actions") {
                for action in array(actions) {
                    self.step(action);
                }
            }
        }
        optional(descriptor, "returns")
            .cloned()
            .unwrap_or(Value::Undefined)
    }

    fn step(&self, step: &Value) {
        match string(field(step, "op")) {
            "on" | "one" => {
                let handler = self.handler(string(field(step, "handler")));
                let event_types = string(field(step, "types"));
                let data = optional(step, "data").cloned().unwrap_or(Value::Undefined);
                if string(field(step, "op")) == "one" {
                    self.hub
                        .one(&PlainOwner, &0, event_types, None, data, handler)
                        .unwrap();
                } else {
                    self.hub
                        .on(&PlainOwner, &0, event_types, None, data, handler)
                        .unwrap();
                }
            }
            "off" => {
                let handler = optional(step, "handler").map(|id| self.handler(string(id)));
                self.hub.off(
                    &0,
                    optional(step, "types").map(string).unwrap_or(""),
                    None,
                    handler.as_ref(),
                );
            }
            "trigger" => {
                let event_type = string(field(step, "types"));
                let only_handlers = bool_field(step, "onlyHandlers");
                let args = optional(step, "args").map(array).unwrap_or(&[]);
                let mut event = Event::new(event_type, 0).unwrap();
                let result = if only_handlers {
                    self.hub
                        .trigger_handler_with(&PlainOwner, &0, &mut event, args)
                } else {
                    self.hub.trigger_with(&PlainOwner, &0, &mut event, args)
                }
                .unwrap();
                let mut entry = vec![
                    ("kind", "trigger".into()),
                    ("type", event_type.into()),
                    ("onlyHandlers", only_handlers.into()),
                    ("result", encode(result)),
                ];
                entry.extend(flags(&event));
                entry.push(("remaining", Value::Number(self.hub.len(&0) as f64)));
                self.trace.borrow_mut().push(object(entry));
            }
            "snapshot" => self.trace.borrow_mut().push(object([
                ("kind", "snapshot".into()),
                ("remaining", Value::Number(self.hub.len(&0) as f64)),
            ])),
            action => panic!("unknown oracle action {action}"),
        }
    }
}

fn flags(event: &Event<usize>) -> [(&'static str, Value); 3] {
    [
        ("defaultPrevented", event.is_default_prevented().into()),
        ("propagationStopped", event.is_propagation_stopped().into()),
        (
            "immediatePropagationStopped",
            event.is_immediate_propagation_stopped().into(),
        ),
    ]
}

#[test]
fn native_events_match_original_upstream_dispatch() {
    let fixture = rjqry::data::parse_json(include_str!("fixtures/events.json")).unwrap();
    let cases = array(field(&fixture, "cases"));
    assert!(cases.len() >= 50);
    for (index, case) in cases.iter().enumerate() {
        let input = field(case, "input");
        let runner = Runner::new(input);
        for step in array(field(input, "steps")) {
            runner.step(step);
        }
        let result = Value::Array(runner.trace.borrow().clone());
        assert!(
            equivalent(&result, field(case, "result")),
            "event case {index}: {}\n native: {result:#?}\n upstream: {:#?}",
            string(field(case, "name")),
            field(case, "result")
        );
    }
}
