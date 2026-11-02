//! Portable algorithms translated from jquery-main/src/core.js, core/access.js,
//! core/stripAndCollapse.js and selector/escapeSelector.js.
use crate::Value;
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

pub fn html_space(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\x0c' | '\r' | ' ')
}
/// Exact-key HTML property fixes from `src/attributes/prop.js`. XML elements
/// bypass this table; `removeProp` uses it unconditionally, like upstream.
pub fn prop_fix(name: &str) -> &str {
    match name {
        "for" => "htmlFor",
        "class" => "className",
        "tabindex" => "tabIndex",
        "readonly" => "readOnly",
        "maxlength" => "maxLength",
        "cellspacing" => "cellSpacing",
        "cellpadding" => "cellPadding",
        "rowspan" => "rowSpan",
        "colspan" => "colSpan",
        "usemap" => "useMap",
        "frameborder" => "frameBorder",
        "contenteditable" => "contentEditable",
        _ => name,
    }
}
/// Boolean equivalent of `expr.match.needsContext` in `src/selector.js`.
/// This deliberately scans the raw selector, as the upstream regular
/// expression does; it is a dispatch guard, not a CSS parser.
pub fn needs_context(selector: &str) -> bool {
    if selector
        .trim_start_matches(html_space)
        .starts_with(['>', '+', '~'])
    {
        return true;
    }
    let bytes = selector.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b':' {
            continue;
        }
        let after_colon = &bytes[index + 1..];
        for name in [
            b"even".as_slice(),
            b"odd",
            b"eq",
            b"gt",
            b"lt",
            b"nth",
            b"first",
            b"last",
        ] {
            if after_colon.len() >= name.len()
                && after_colon[..name.len()].eq_ignore_ascii_case(name)
                && after_colon.get(name.len()) != Some(&b'-')
            {
                // Upstream's optional parenthesized argument also has an
                // empty alternative, followed by (?=[^-]|$). Thus arguments
                // do not change this boolean prefix/next-character test.
                return true;
            }
        }
    }
    false
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrayError {
    LengthLimit,
}
/// jQuery.makeArray for owned data. Plain objects can be array-like; strings
/// remain single values. Missing intermediate indices become Undefined. The
/// caller provides an allocation bound instead of trusting an arbitrary length.
pub fn make_array(value: &Value, max_length: usize) -> Result<Vec<Value>, ArrayError> {
    let values = match value {
        Value::Null | Value::Undefined => Vec::new(),
        Value::Array(values) => {
            if values.len() > max_length {
                return Err(ArrayError::LengthLimit);
            }
            values.clone()
        }
        Value::Object(_) => match value.object_get("length") {
            Some(Value::Number(0.0)) => Vec::new(),
            Some(Value::Number(n))
                if *n > 0.0
                    && value
                        .object_get(&crate::value::number_to_string(n - 1.0))
                        .is_some() =>
            {
                if !n.is_finite() || *n > max_length as f64 {
                    return Err(ArrayError::LengthLimit);
                }
                let length = *n as usize;
                let length = length
                    .checked_add(usize::from((length as f64) < *n))
                    .ok_or(ArrayError::LengthLimit)?;
                if length > max_length {
                    return Err(ArrayError::LengthLimit);
                }
                (0..length)
                    .map(|i| {
                        value
                            .object_get(&i.to_string())
                            .cloned()
                            .unwrap_or(Value::Undefined)
                    })
                    .collect()
            }
            _ => alloc::vec![value.clone()],
        },
        _ => alloc::vec![value.clone()],
    };
    if values.len() > max_length {
        return Err(ArrayError::LengthLimit);
    }
    Ok(values)
}
pub fn each_object<'a>(
    value: &'a Value,
    mut callback: impl FnMut(&str, &Value) -> bool,
) -> &'a Value {
    for (key, value) in value.ordered_entries() {
        if !callback(key, value) {
            break;
        }
    }
    value
}
pub fn map_object(
    value: &Value,
    mut callback: impl FnMut(&Value, &str) -> Option<Value>,
) -> Vec<Value> {
    let mut result = Vec::new();
    for (key, value) in value.ordered_entries() {
        match callback(value, key) {
            None | Some(Value::Null | Value::Undefined) => {}
            Some(Value::Array(values)) => result.extend(values),
            Some(value) => result.push(value),
        }
    }
    result
}
pub fn strip_and_collapse(text: &str) -> String {
    text.split(html_space)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn to_type(value: &Value) -> &'static str {
    match value {
        Value::Undefined => "undefined",
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
pub fn is_plain_object(value: &Value) -> bool {
    matches!(value, Value::Object(_))
}
pub fn is_empty_object(value: &Value) -> bool {
    value.ordered_entries().is_empty()
        && !matches!(value,Value::Array(values) if !values.is_empty())
        && !matches!(value,Value::String(text) if !text.is_empty())
}
pub fn grep<T: Clone>(
    values: &[T],
    mut predicate: impl FnMut(&T, usize) -> bool,
    invert: bool,
) -> Vec<T> {
    values
        .iter()
        .enumerate()
        .filter(|(i, value)| predicate(value, *i) != invert)
        .map(|(_, value)| value.clone())
        .collect()
}
/// Rust callbacks preserve the upstream early-stop rule without a JS function
/// representation: false stops iteration and the original input is returned.
pub fn each<T>(values: &[T], mut callback: impl FnMut(usize, &T) -> bool) -> &[T] {
    for (i, value) in values.iter().enumerate() {
        if !callback(i, value) {
            break;
        }
    }
    values
}
pub fn map(
    values: &[Value],
    mut callback: impl FnMut(&Value, usize) -> Option<Value>,
) -> Vec<Value> {
    let mut result = Vec::new();
    for (i, value) in values.iter().enumerate() {
        match callback(value, i) {
            None | Some(Value::Null | Value::Undefined) => {}
            Some(Value::Array(values)) => result.extend(values),
            Some(value) => result.push(value),
        }
    }
    result
}
pub fn merge<T: Clone>(first: &mut Vec<T>, second: &[T]) {
    first.extend_from_slice(second);
}
pub fn in_array<T: PartialEq>(value: &T, values: &[T], from: isize) -> Option<usize> {
    let start = if from < 0 {
        values.len().saturating_sub(from.unsigned_abs())
    } else {
        from as usize
    };
    values
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, current)| *current == value)
        .map(|(index, _)| index)
}

/// CSS.escape's algorithm used by jQuery.escapeSelector; trailing spaces after
/// hex escapes disambiguate following hexadecimal characters.
pub fn escape_selector(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut output = String::new();
    for (i, &character) in chars.iter().enumerate() {
        let code = character as u32;
        if code == 0 {
            output.push('\u{fffd}');
        } else if (1..=31).contains(&code)
            || code == 127
            || (i == 0 && character.is_ascii_digit())
            || (i == 1 && chars[0] == '-' && character.is_ascii_digit())
        {
            output.push_str(&format!("\\{code:x} "));
        } else if (i == 0 && character == '-' && chars.len() == 1)
            || !(code >= 128
                || character == '-'
                || character == '_'
                || character.is_ascii_alphanumeric())
        {
            output.push('\\');
            output.push(character);
        } else {
            output.push(character);
        }
    }
    output
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtendError {
    DepthLimit,
    UnrepresentableUtf16,
    NamedArrayProperty,
    ResourceLimit,
}
/// Deep merge of owned data values; no getter execution, prototypes or function
/// dispatch is implied. Object/array ordering, undefined skips, array tails and
/// the __proto__ guard follow jQuery.extend.
pub fn extend(target: &mut Value, sources: &[Value], deep: bool) -> Result<(), ExtendError> {
    extend_at(target, sources, deep, 0)
}
fn extend_at(
    target: &mut Value,
    sources: &[Value],
    deep: bool,
    depth: usize,
) -> Result<(), ExtendError> {
    if depth > 64 {
        return Err(ExtendError::DepthLimit);
    }
    if !matches!(target, Value::Object(_) | Value::Array(_)) {
        *target = Value::Object(Vec::new());
    }
    for source in sources {
        let entries: Vec<(String, Value)> = match source {
            Value::Object(_) => source
                .ordered_entries()
                .into_iter()
                .map(|(key, value)| (key.into(), value.clone()))
                .collect(),
            Value::Array(values) => values
                .iter()
                .enumerate()
                .map(|(i, value)| (i.to_string(), value.clone()))
                .collect(),
            Value::String(text) => {
                if text.chars().any(|c| c as u32 > 0xffff) {
                    return Err(ExtendError::UnrepresentableUtf16);
                }
                text.chars()
                    .enumerate()
                    .map(|(i, c)| (i.to_string(), Value::String(c.to_string())))
                    .collect()
            }
            _ => Vec::new(),
        };
        for (name, copy) in entries {
            if name == "__proto__" || copy == Value::Undefined {
                continue;
            }
            let previous = match target {
                Value::Object(_) => target.object_get(&name).cloned(),
                Value::Array(values) => name
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| values.get(i).cloned()),
                _ => None,
            };
            let value = if deep && matches!(copy, Value::Object(_) | Value::Array(_)) {
                let mut next = match (&copy, previous) {
                    (Value::Array(_), Some(value @ Value::Array(_)))
                    | (Value::Object(_), Some(value @ Value::Object(_))) => value,
                    (Value::Array(_), _) => Value::Array(Vec::new()),
                    _ => Value::Object(Vec::new()),
                };
                extend_at(&mut next, &[copy], true, depth + 1)?;
                next
            } else {
                copy
            };
            if let Value::Array(values) = target {
                if let Ok(i) = name.parse::<usize>() {
                    if i.to_string() != name || i >= u32::MAX as usize {
                        return Err(ExtendError::NamedArrayProperty);
                    }
                    if i > 1_000_000 {
                        return Err(ExtendError::ResourceLimit);
                    }
                    values.resize(values.len().max(i + 1), Value::Undefined);
                    values[i] = value;
                } else {
                    return Err(ExtendError::NamedArrayProperty);
                }
                // Named properties on JavaScript arrays require a binding-level
                // object model; owned Value::Array deliberately stores indices.
            } else {
                target.set_property(name, value);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    #[test]
    fn array_like_lengths_are_bounded_and_object_callbacks_use_property_order() {
        assert_eq!(
            make_array(&Value::Array(vec![1.into(), 2.into()]), 1),
            Err(ArrayError::LengthLimit)
        );
        assert_eq!(make_array(&Value::Undefined, 0), Ok(vec![]));
        let value = Value::Object(vec![
            ("length".into(), 1000000.into()),
            ("999999".into(), 1.into()),
        ]);
        assert_eq!(make_array(&value, 10), Err(ArrayError::LengthLimit));
        let object = Value::Object(vec![
            ("b".into(), 3.into()),
            ("2".into(), 2.into()),
            ("1".into(), 1.into()),
        ]);
        let mut keys = Vec::new();
        each_object(&object, |key, _| {
            keys.push(String::from(key));
            keys.len() < 2
        });
        assert_eq!(keys, ["1", "2"]);
        assert_eq!(
            map_object(&object, |value, key| if key == "2" {
                None
            } else {
                Some(Value::Array(vec![value.clone(), Value::Null]))
            }),
            vec![1.into(), Value::Null, 3.into(), Value::Null]
        );
    }
    #[test]
    fn css_escapes_and_html_whitespace_match_upstream_rules() {
        assert_eq!(escape_selector("0a #x\0"), "\\30 a\\ \\#x�");
        assert_eq!(escape_selector("-"), "\\-");
        assert_eq!(escape_selector("-1"), "-\\31 ");
        assert_eq!(strip_and_collapse(" a\t\r\nb \u{a0} c "), "a b \u{a0} c");
    }
    #[test]
    fn merge_preserves_array_tail_skips_undefined_and_blocks_proto() {
        let mut target = Value::Object(vec![(
            "a".into(),
            Value::Array(vec![Value::Number(1.0), Value::Number(2.0)]),
        )]);
        extend(
            &mut target,
            &[Value::Object(vec![
                ("a".into(), Value::Array(vec![Value::Number(3.0)])),
                ("__proto__".into(), Value::Bool(true)),
                ("missing".into(), Value::Undefined),
            ])],
            true,
        )
        .unwrap();
        assert_eq!(
            target.object_get("a"),
            Some(&Value::Array(vec![Value::Number(3.0), Value::Number(2.0)]))
        );
        assert!(target.object_get("__proto__").is_none());
        assert!(target.object_get("missing").is_none());
    }
    #[test]
    fn context_guard_matches_upstream_raw_selector_regex() {
        for selector in [
            "> p",
            " \t+ p",
            "~ p",
            ":first",
            "p:LAST",
            ":eq(-2)",
            ":odd()",
            ":nth( )",
            ":firstborn",
            "[data-x=':first']",
            "\\:first",
        ] {
            assert!(needs_context(selector), "{selector}");
        }
        for selector in [
            "",
            "p",
            ":first-child",
            ":last-of-type",
            ":nth-child(2)",
            ":nth-last-child(2)",
            ":EQ-thing",
            "p > a",
            "\u{a0}> p",
            ":not(.item)",
        ] {
            assert!(!needs_context(selector), "{selector}");
        }
    }
}
