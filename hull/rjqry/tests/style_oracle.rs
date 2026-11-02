//! Expectations are produced by tools/style_oracle.mjs from vendored source.
//! The only numerical tolerance is for the documented portable swing cosine.
use rjqry::{Value, effects, style};

fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    value.object_get(name).unwrap()
}
fn string(value: &Value) -> &str {
    let Value::String(value) = value else {
        panic!("expected string, got {value:?}")
    };
    value
}
fn number(value: &Value) -> f64 {
    let Value::Number(value) = value else {
        panic!("expected number, got {value:?}")
    };
    *value
}
fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    )
}
fn decode(value: &Value) -> Value {
    if let Some(Value::String(number)) = value.object_get("$number") {
        return Value::Number(match number.as_str() {
            "NaN" => f64::NAN,
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            _ => panic!("unknown number marker"),
        });
    }
    if value.object_get("$undefined") == Some(&Value::Bool(true)) {
        return Value::Undefined;
    }
    match value {
        Value::Array(values) => Value::Array(values.iter().map(decode).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(name, value)| (name.clone(), decode(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}
fn encode(value: Value) -> Value {
    match value {
        Value::Number(number) if !number.is_finite() => object([(
            "$number",
            Value::String(rjqry::value::number_to_string(number)),
        )]),
        Value::Undefined => object([("$undefined", Value::Bool(true))]),
        Value::Array(values) => Value::Array(values.into_iter().map(encode).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(name, value)| (name, encode(value)))
                .collect(),
        ),
        other => other,
    }
}
fn equivalent(left: &Value, right: &Value, tolerance: f64) -> bool {
    match (left, right) {
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter().all(|(name, value)| {
                    right
                        .object_get(name)
                        .is_some_and(|other| equivalent(value, other, tolerance))
                })
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equivalent(a, b, tolerance))
        }
        (Value::Number(a), Value::Number(b)) if tolerance > 0.0 => {
            a == b || (a - b).abs() <= tolerance * (1.0 + a.abs().max(b.abs()))
        }
        _ => left == right,
    }
}
fn speed_value(options: effects::SpeedOptions) -> Value {
    object([
        ("duration_ms", Value::Number(options.duration_ms)),
        ("easing", options.easing),
        ("queue", options.queue),
        ("completion", options.completion),
        (
            "extra_options",
            Value::Array(
                options
                    .extra_options
                    .into_iter()
                    .map(|(name, value)| Value::Array(vec![Value::String(name), value]))
                    .collect(),
            ),
        ),
    ])
}

#[test]
fn css_and_effects_match_recorded_upstream_algorithms() {
    let fixture = rjqry::data::parse_json(include_str!("fixtures/style.json")).unwrap();
    let Value::Array(cases) = field(&fixture, "cases") else {
        panic!("expected cases")
    };
    assert!(cases.len() >= 1_000);
    for (index, case) in cases.iter().enumerate() {
        let op = string(field(case, "op"));
        let input = decode(field(case, "input"));
        let result = match op {
            "css_camel" => Value::String(style::css_camel_case(string(&input))),
            "auto_px" => Value::Bool(style::is_auto_px(string(&input))),
            "css_number" => match style::parse_css_number(string(&input)) {
                None => Value::Null,
                Some(parsed) => object([
                    (
                        "relative",
                        match parsed.relative {
                            Some(style::RelativeOperator::Add) => Value::String("+".into()),
                            Some(style::RelativeOperator::Subtract) => Value::String("-".into()),
                            None => Value::Null,
                        },
                    ),
                    ("number", Value::Number(parsed.number)),
                    ("number_text", Value::String(parsed.number_text.into())),
                    ("unit", Value::String(parsed.unit.into())),
                ]),
            },
            "endpoint" => Value::Number(
                style::parse_css_number(string(field(&input, "target")))
                    .unwrap()
                    .endpoint(number(field(&input, "initial"))),
            ),
            "speed" => speed_value(effects::normalize_speed(
                field(&input, "speed"),
                None,
                None,
                field(&input, "off") == &Value::Bool(true),
            )),
            "speed_args" => speed_value(effects::normalize_speed(
                field(&input, "speed"),
                Some(field(&input, "easing")),
                Some(field(&input, "complete")),
                false,
            )),
            "queue" => effects::normalize_queue(&input),
            "tween" => {
                let mut tween = effects::Tween::new(
                    "__oracle__",
                    number(field(&input, "start")),
                    number(field(&input, "end")),
                    number(field(&input, "duration")),
                    effects::Easing::from_name(string(field(&input, "easing"))),
                    None,
                );
                tween.run(number(field(&input, "percent"))).unwrap();
                object([
                    ("now", Value::Number(tween.now)),
                    ("pos", Value::Number(tween.pos.unwrap())),
                ])
            }
            "swing" => Value::Number(effects::swing(number(&input)).unwrap()),
            "progress" => {
                let progress = effects::animation_progress(
                    number(field(&input, "start")),
                    number(field(&input, "duration")),
                    number(field(&input, "now")),
                );
                object([
                    ("percent", Value::Number(progress.percent)),
                    ("remaining_ms", Value::Number(progress.remaining_ms)),
                ])
            }
            _ => panic!("unknown source operation {op}"),
        };
        let result = encode(result);
        let expected = field(case, "result");
        let tolerance = if matches!(op, "tween" | "swing") {
            2e-15
        } else {
            0.0
        };
        assert!(
            equivalent(&result, expected, tolerance),
            "case {index} {op} input {input:?}\n native: {result:?}\n upstream: {expected:?}"
        );
    }
}
