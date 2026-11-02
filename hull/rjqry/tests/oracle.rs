//! Recorded results come from tools/oracle.mjs executing original source. This
//! test also exercises the native JSON reader, without test-only dependencies.
use rjqry::{
    Dom, Error, JQuery, NodeKind, Value,
    callbacks::{Callback, CallbackList, CallbackOptions},
    deferred::{Deferred, State, TaskQueue},
    queue::Queue,
};
use std::{cell::RefCell, rc::Rc};
fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn array(values: impl IntoIterator<Item = Value>) -> Value {
    Value::Array(values.into_iter().collect())
}
fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.object_get(key).unwrap()
}
fn string(value: &Value) -> &str {
    let Value::String(text) = value else {
        panic!("expected string: {value:?}")
    };
    text
}
fn number(value: &Value) -> f64 {
    let Value::Number(n) = value else {
        panic!("expected number")
    };
    *n
}
fn numbers(value: &Value) -> Vec<i32> {
    let Value::Array(values) = value else {
        panic!("expected array")
    };
    values.iter().map(|v| number(v) as i32).collect()
}
fn encode(value: Value) -> Value {
    match value {
        Value::Number(n) if !n.is_finite() => {
            object([("$number", rjqry::value::number_to_string(n).into())])
        }
        Value::Undefined => object([("$undefined", true.into())]),
        Value::Array(values) => array(values.into_iter().map(encode)),
        Value::Object(values) => {
            Value::Object(values.into_iter().map(|(k, v)| (k, encode(v))).collect())
        }
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
struct ArrayHost;
impl Dom for ArrayHost {
    type Node = i32;
    fn query(&self, _: Option<&i32>, _: &str) -> Result<Vec<i32>, Error> {
        Err(Error::Unsupported("array fixture"))
    }
    fn matches(&self, _: &i32, _: &str) -> Result<bool, Error> {
        Err(Error::Unsupported("array fixture"))
    }
    fn kind(&self, _: &i32) -> NodeKind {
        NodeKind::Element
    }
    fn parent(&self, _: &i32) -> Option<i32> {
        None
    }
    fn children(&self, _: &i32) -> Vec<i32> {
        vec![]
    }
    fn unique_sort(&self, _: &mut Vec<i32>) {}
    fn text(&self, _: &i32) -> String {
        String::new()
    }
}
fn callback_case(options: &str) -> Value {
    let trace = Rc::new(RefCell::new(Vec::<String>::new()));
    let make = |name: &'static str, returns: bool| {
        let trace = trace.clone();
        Callback::new(move |context: &String, value: &i32| {
            trace.borrow_mut().push(format!("{name}:{context}:{value}"));
            returns
        })
    };
    let a = make("a", true);
    let b = make("b", false);
    let c = make("c", true);
    let d = make("d", true);
    let list = CallbackList::new(CallbackOptions::parse(options));
    list.add_many([a.clone(), b, a.clone()]);
    list.fire_with("one".into(), 1);
    list.remove(&a);
    list.add(c);
    list.fire_with("two".into(), 2);
    list.lock();
    list.add(d);
    list.fire_with("three".into(), 3);
    let values = trace
        .borrow()
        .iter()
        .cloned()
        .map(Value::from)
        .collect::<Vec<_>>();
    object([
        ("trace", array(values)),
        ("disabled", list.disabled().into()),
        ("locked", list.locked().into()),
        ("fired", list.fired().into()),
        ("has", list.has_any().into()),
    ])
}
fn deferred_case() -> Value {
    let trace = Rc::new(RefCell::new(Vec::<String>::new()));
    let d = Deferred::<i32>::new();
    let tasks = TaskQueue::new();
    let capture = trace.clone();
    d.progress(move |x| capture.borrow_mut().push(format!("progress:{x}")));
    let capture = trace.clone();
    d.done(move |x| capture.borrow_mut().push(format!("done:{x}")));
    let capture = trace.clone();
    let child = d.promise().then(&tasks, move |x| {
        capture.borrow_mut().push(format!("then:{x}"));
        Ok(x + 1)
    });
    let capture = trace.clone();
    child.done(move |x| capture.borrow_mut().push(format!("child:{x}")));
    d.notify(2);
    d.resolve(7);
    d.reject(8);
    d.notify(3);
    let capture = trace.clone();
    d.done(move |x| capture.borrow_mut().push(format!("late:{x}")));
    trace.borrow_mut().push("before-drain".into());
    tasks.drain(100);
    let values = trace
        .borrow()
        .iter()
        .cloned()
        .map(Value::from)
        .collect::<Vec<_>>();
    object([
        ("trace", array(values)),
        (
            "state",
            match d.state() {
                State::Pending => "pending",
                State::Resolved => "resolved",
                State::Rejected => "rejected",
            }
            .into(),
        ),
    ])
}
fn queue_case(kind: &str) -> Value {
    let q = Queue::new(kind);
    q.replace([1, 2]);
    let trace = Rc::new(RefCell::new(Vec::<String>::new()));
    let capture = trace.clone();
    q.empty_callbacks().add(Callback::new(move |_, _| {
        capture.borrow_mut().push("empty".into());
        true
    }));
    let mut lengths = vec![Value::Number(q.len() as f64)];
    for _ in 0..3 {
        if let Some(id) = q.dequeue() {
            trace.borrow_mut().push(format!("job:{id}"));
        }
        lengths.push(Value::Number(q.len() as f64));
    }
    let values = trace
        .borrow()
        .iter()
        .cloned()
        .map(Value::from)
        .collect::<Vec<_>>();
    object([("trace", array(values)), ("lengths", array(lengths))])
}
#[test]
fn native_matches_recorded_upstream() {
    let fixture = rjqry::data::parse_json(include_str!("fixtures/portable.json")).unwrap();
    let Value::Array(cases) = field(&fixture, "cases") else {
        panic!()
    };
    assert!(cases.len() >= 700);
    for (index, case) in cases.iter().enumerate() {
        let op = string(field(case, "op"));
        let input = field(case, "input");
        let result = match op {
            "make_array" => encode(Value::Array(
                rjqry::utilities::make_array(input, 1000).unwrap(),
            )),
            "collapse" => rjqry::utilities::strip_and_collapse(string(input)).into(),
            "data" => encode(rjqry::data::parse_data_attribute(string(input)).unwrap()),
            "camel" => rjqry::data::camel_case(string(input)).into(),
            "escape" => rjqry::utilities::escape_selector(string(input)).into(),
            "number" => rjqry::value::number_to_string(number(input)).into(),
            "param" => rjqry::serialize::param(
                field(input, "value"),
                field(input, "traditional") == &Value::Bool(true),
            )
            .unwrap()
            .into(),
            "extend" => {
                let mut result = Value::Object(vec![]);
                rjqry::utilities::extend(
                    &mut result,
                    &[
                        field(input, "target").clone(),
                        field(input, "source").clone(),
                    ],
                    field(input, "deep") == &Value::Bool(true),
                )
                .unwrap();
                encode(result)
            }
            "collection" => {
                let jq = JQuery::new(ArrayHost);
                let values = jq.from_nodes(numbers(field(input, "nodes")));
                let index = number(field(input, "index")) as isize;
                let nodes = |values: Vec<i32>| array(values.into_iter().map(Value::from));
                object([
                    (
                        "get",
                        values.get(index).map_or(Value::Null, |n| Value::from(*n)),
                    ),
                    ("eq", nodes(values.eq(index).to_array())),
                    ("first", nodes(values.first().to_array())),
                    ("last", nodes(values.last().to_array())),
                    ("even", nodes(values.even().to_array())),
                    ("odd", nodes(values.odd().to_array())),
                    ("slice", nodes(values.slice(index, Some(-1)).to_array())),
                    ("end", nodes(values.eq(index).end().to_array())),
                ])
            }
            "callbacks" => callback_case(string(input)),
            "deferred" => deferred_case(),
            "queue" => queue_case(string(input)),
            _ => panic!("unknown operation {op}"),
        };
        assert!(
            equivalent(&result, field(case, "result")),
            "case {index} {op} input {input:?}\n native: {result:?}\n upstream: {:?}",
            field(case, "result")
        );
    }
}
