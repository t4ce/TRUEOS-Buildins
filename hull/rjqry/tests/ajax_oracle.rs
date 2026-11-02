//! Fixtures execute the unchanged ajax.js helper functions through
//! tools/ajax_oracle.mjs. No browser or network participates on either side.
use rjqry::{
    Value,
    ajax::{
        ConversionOptions, ConversionState, Converter, Registry, convert_response, extend_settings,
        select_response,
    },
};
use std::{cell::RefCell, rc::Rc};

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
fn optional<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.object_get(key)
}
fn string(value: &Value) -> &str {
    let Value::String(text) = value else {
        panic!("expected string: {value:?}")
    };
    text
}
fn array(value: &Value) -> &[Value] {
    let Value::Array(values) = value else {
        panic!("expected array: {value:?}")
    };
    values
}
fn strings(value: &Value) -> Vec<String> {
    array(value)
        .iter()
        .map(|value| string(value).to_string())
        .collect()
}
fn bool_field(value: &Value, key: &str) -> bool {
    optional(value, key) == Some(&Value::Bool(true))
}
fn value_array(values: impl IntoIterator<Item = Value>) -> Value {
    Value::Array(values.into_iter().collect())
}
fn string_array(values: impl IntoIterator<Item = String>) -> Value {
    value_array(values.into_iter().map(Value::from))
}

fn encode(value: Value) -> Value {
    match value {
        Value::Undefined => object([("$undefined", true.into())]),
        Value::Array(values) => value_array(values.into_iter().map(encode)),
        Value::Object(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key, encode(value)))
                .collect(),
        ),
        value => value,
    }
}
fn decode(value: &Value) -> Value {
    match value {
        Value::Object(_) if optional(value, "$undefined") == Some(&Value::Bool(true)) => {
            Value::Undefined
        }
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), decode(value)))
                .collect(),
        ),
        Value::Array(values) => value_array(values.iter().map(decode)),
        value => value.clone(),
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

fn transform(descriptor: &Value, response: Value) -> Result<Value, Value> {
    let value = || decode(field(descriptor, "value"));
    match string(field(descriptor, "kind")) {
        "append" => Ok(Value::String(
            response.js_string() + string(field(descriptor, "value")),
        )),
        "add" => {
            let number = match response {
                Value::Number(number) => number,
                Value::String(text) => text.parse::<f64>().unwrap(),
                _ => panic!("fixture numeric input"),
            };
            let Value::Number(offset) = field(descriptor, "value") else {
                panic!()
            };
            Ok(Value::Number(number + offset))
        }
        "wrap" => Ok(object([("value", response), ("tag", value())])),
        "constant" => Ok(value()),
        "throw" => Err(value()),
        kind => panic!("unknown fixture transform {kind}"),
    }
}

fn conversion_options(input: &Value, trace: &Rc<RefCell<Vec<Value>>>) -> ConversionOptions {
    let mut options = ConversionOptions {
        data_types: strings(field(input, "dataTypes")),
        data_type: optional(input, "dataType").map(string).unwrap_or("").into(),
        throws: bool_field(input, "throws"),
        ..ConversionOptions::default()
    };
    if let Some(fields) = optional(input, "responseFields") {
        options.response_fields = array(fields)
            .iter()
            .map(|pair| {
                let pair = array(pair);
                (string(&pair[0]).into(), string(&pair[1]).into())
            })
            .collect();
    }
    if let Some(converters) = optional(input, "converters") {
        for descriptor in array(converters) {
            let key = string(field(descriptor, "key")).to_string();
            let converter = if string(field(descriptor, "kind")) == "identity" {
                Converter::Identity
            } else {
                let descriptor = descriptor.clone();
                let trace = trace.clone();
                let key = key.clone();
                Converter::new(move |response| {
                    trace.borrow_mut().push(object([
                        ("stage", key.clone().into()),
                        ("response", encode(response.clone())),
                    ]));
                    transform(&descriptor, response)
                })
            };
            options.converters.push((key, converter));
        }
    }
    if let Some(filter) = optional(input, "filter") {
        let filter = filter.clone();
        let trace = trace.clone();
        options.data_filter = Some(Rc::new(move |response, data_type| {
            trace.borrow_mut().push(object([
                ("stage", "filter".into()),
                ("response", encode(response.clone())),
                ("dataType", data_type.into()),
            ]));
            transform(&filter, response)
        }));
    }
    options
}

fn conversion(input: &Value) -> Value {
    let trace = Rc::new(RefCell::new(Vec::new()));
    let options = conversion_options(input, &trace);
    let converted = convert_response(
        &options,
        decode(field(input, "response")),
        optional(input, "isSuccess") != Some(&Value::Bool(false)),
    );
    let (result, fields) = match converted {
        Ok(response) => {
            let result = match response.state {
                ConversionState::Success => object([
                    ("state", "success".into()),
                    ("data", encode(response.data.unwrap())),
                ]),
                ConversionState::ParserError => object([
                    ("state", "parsererror".into()),
                    ("error", encode(response.error.unwrap())),
                ]),
            };
            (
                result,
                Value::Object(
                    response
                        .response_fields
                        .into_iter()
                        .map(|(key, value)| (key, encode(value)))
                        .collect(),
                ),
            )
        }
        Err(failure) => (
            object([("thrown", encode(failure.error))]),
            Value::Object(
                failure
                    .response_fields
                    .into_iter()
                    .map(|(key, value)| (key, encode(value)))
                    .collect(),
            ),
        ),
    };
    let trace = Value::Array(trace.borrow().clone());
    object([("result", result), ("fields", fields), ("trace", trace)])
}

fn selection(input: &Value) -> Value {
    let mut options = ConversionOptions {
        data_types: strings(field(input, "dataTypes")),
        ..ConversionOptions::default()
    };
    if let Some(keys) = optional(input, "converterKeys") {
        options.converters = strings(keys)
            .into_iter()
            .map(|key| (key, Converter::Identity))
            .collect();
    }
    let responses: Vec<_> = array(field(input, "responses"))
        .iter()
        .map(|pair| {
            let pair = array(pair);
            (string(&pair[0]).to_string(), decode(&pair[1]))
        })
        .collect();
    let content_matches: Vec<_> = array(field(input, "contents"))
        .iter()
        .map(|item| {
            (
                string(field(item, "type")).to_string(),
                bool_field(item, "matched"),
            )
        })
        .collect();
    let content_type = optional(input, "contentType").map(string);
    let mime_override = optional(input, "mimeOverride").map(string);
    let response = select_response(
        &mut options,
        &responses,
        content_type,
        mime_override,
        &content_matches,
    );
    object([
        ("response", encode(response.unwrap_or(Value::Undefined))),
        ("dataTypes", string_array(options.data_types)),
    ])
}

fn registry(input: &Value) -> Value {
    let hooks = array(field(input, "hooks"));
    let mut registry = Registry::new();
    for (index, hook) in hooks.iter().enumerate() {
        registry.add(
            optional(hook, "expression")
                .filter(|value| **value != Value::Null)
                .map(string),
            index,
        );
    }
    let trace = RefCell::new(Vec::new());
    let mut data_types = strings(field(input, "dataTypes"));
    let is_transport = string(field(input, "mode")) == "transport";
    let inspect = |index: &usize, types: &mut Vec<String>| {
        let hook = &hooks[*index];
        trace.borrow_mut().push(object([
            ("id", field(hook, "id").clone()),
            ("dataTypes", string_array(types.clone())),
        ]));
        if let Some(append) = optional(hook, "append") {
            types.push(string(append).into());
        }
        hook
    };
    let selected = if is_transport {
        let first = data_types.first().cloned().unwrap_or_default();
        registry.inspect_transports(&first, |index| {
            let hook = inspect(index, &mut data_types);
            bool_field(hook, "transport")
                .then(|| object([("transport", field(hook, "id").clone())]))
        })
    } else {
        registry
            .inspect_prefilters(&mut data_types, 100, |index, types| {
                optional(inspect(index, types), "redirect").map(|value| string(value).to_string())
            })
            .unwrap();
        None
    };
    let mut fields = vec![
        ("trace", Value::Array(trace.into_inner())),
        ("dataTypes", string_array(data_types)),
    ];
    if is_transport {
        fields.push(("selected", selected.unwrap_or(Value::Null)));
    }
    object(fields)
}

#[test]
fn native_ajax_helpers_match_original_source() {
    let fixture = rjqry::data::parse_json(include_str!("fixtures/ajax.json")).unwrap();
    let cases = array(field(&fixture, "cases"));
    assert!(cases.len() >= 60);
    for (index, case) in cases.iter().enumerate() {
        let input = field(case, "input");
        let actual = match string(field(case, "op")) {
            "settings" => {
                let mut target = decode(field(input, "target"));
                let flat_strings = strings(field(input, "flat"));
                let flat = flat_strings.iter().map(String::as_str).collect::<Vec<_>>();
                extend_settings(&mut target, &decode(field(input, "source")), &flat).unwrap();
                encode(target)
            }
            "conversion" => conversion(input),
            "selection" => selection(input),
            "registry" => registry(input),
            op => panic!("unknown Ajax fixture operation {op}"),
        };
        assert!(
            equivalent(&actual, field(case, "result")),
            "Ajax case {index}: {}\n native: {actual:#?}\n upstream: {:#?}",
            string(field(case, "name")),
            field(case, "result")
        );
    }
}
