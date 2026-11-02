//! Native translation of jQuery 4 `src/serialize.js`: `buildParams`, `param`,
//! URI component encoding, and the pure form-value newline conversion.
//!
//! Browser form discovery, disabled-state matching, and `.val()` require the
//! DOM host. Callable parameter values likewise must be evaluated by the script
//! host before constructing [`Value`], which has no function variant.

use alloc::string::{String, ToString};

use crate::Value;
use crate::value::number_to_string;

/// Explicit native resource policy; these are not additional jQuery options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamLimits {
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_pairs: usize,
    pub max_output_bytes: usize,
}

impl Default for ParamLimits {
    fn default() -> Self {
        Self {
            max_depth: 128,
            max_nodes: 100_000,
            max_pairs: 100_000,
            max_output_bytes: 1_048_576,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamError {
    /// Native API admits the documented object/form-entry-array inputs plus
    /// null/undefined. Primitive roots do not have a native property container.
    InvalidRoot,
    /// A null or undefined entry cannot supply the form entry's `.name`.
    InvalidFormEntry {
        index: usize,
    },
    DepthLimit,
    NodeLimit,
    PairLimit,
    OutputLimit,
}

/// `jQuery.param(value, traditional)` over native objects and form-entry arrays.
/// Array roots are form entries (`{name, value}`), not a parameter value list.
/// Objects enumerate integer-index keys first, then other insertion-order keys.
pub fn param(value: &Value, traditional: bool) -> Result<String, ParamError> {
    param_with_limits(value, traditional, ParamLimits::default())
}

pub fn param_with_limits(
    value: &Value,
    traditional: bool,
    limits: ParamLimits,
) -> Result<String, ParamError> {
    let mut writer = ParamWriter::new(limits);
    writer.visit(0)?;
    match value {
        Value::Null | Value::Undefined => {}
        Value::Object(entries) => {
            writer.check_children(entries.len())?;
            for (key, child) in value.ordered_entries() {
                writer.build_params(key, child, traditional, 1)?;
            }
        }
        Value::Array(entries) => {
            writer.check_children(entries.len())?;
            for (index, entry) in entries.iter().enumerate() {
                writer.visit(1)?;
                if matches!(entry, Value::Null | Value::Undefined) {
                    return Err(ParamError::InvalidFormEntry { index });
                }
                // Primitive entries have undefined name/value properties.
                let name = entry.object_get("name").unwrap_or(&Value::Undefined);
                let value = entry.object_get("value").unwrap_or(&Value::Undefined);
                let name = writer.scalar_string(name, 2)?;
                writer.add(&name, value, 2)?;
            }
        }
        _ => return Err(ParamError::InvalidRoot),
    }
    Ok(writer.output)
}

/// Already-discovered form entries, in collection order. Repeated names remain
/// repeated and values are scalar-stringified, as in upstream's array branch.
pub fn param_pairs(pairs: &[(String, Value)]) -> Result<String, ParamError> {
    param_pairs_with_limits(pairs, ParamLimits::default())
}

pub fn param_pairs_with_limits(
    pairs: &[(String, Value)],
    limits: ParamLimits,
) -> Result<String, ParamError> {
    let mut writer = ParamWriter::new(limits);
    writer.check_children(pairs.len())?;
    for (name, value) in pairs {
        writer.add(name, value, 0)?;
    }
    Ok(writer.output)
}

/// JavaScript `encodeURIComponent` over Rust's UTF-8 string domain. The ASCII
/// set `A-Z a-z 0-9 - _ . ! ~ * ' ( )` remains unescaped; spaces become `%20`.
/// Rust strings exclude the unpaired UTF-16 surrogates that throw URIError in JS.
pub fn encode_uri_component(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut result = String::new();
    for byte in input.bytes() {
        if uri_component_unescaped(byte) {
            result.push(char::from(byte));
        } else {
            result.push('%');
            result.push(char::from(HEX[usize::from(byte >> 4)]));
            result.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    result
}

/// The `value.replace(/\r?\n/g, "\r\n")` step of `serializeArray`.
pub fn normalize_form_newlines(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut previous_was_cr = false;
    for ch in input.chars() {
        if ch == '\n' && !previous_was_cr {
            output.push('\r');
        }
        output.push(ch);
        previous_was_cr = ch == '\r';
    }
    output
}

fn uri_component_unescaped(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
        )
}

struct ParamWriter {
    output: String,
    pairs: usize,
    nodes: usize,
    limits: ParamLimits,
}

impl ParamWriter {
    fn new(limits: ParamLimits) -> Self {
        Self {
            output: String::new(),
            pairs: 0,
            nodes: 0,
            limits,
        }
    }

    fn visit(&mut self, depth: usize) -> Result<(), ParamError> {
        if depth > self.limits.max_depth {
            return Err(ParamError::DepthLimit);
        }
        if self.nodes >= self.limits.max_nodes {
            return Err(ParamError::NodeLimit);
        }
        self.nodes += 1;
        Ok(())
    }

    fn check_children(&self, count: usize) -> Result<(), ParamError> {
        if count > self.limits.max_nodes.saturating_sub(self.nodes) {
            return Err(ParamError::NodeLimit);
        }
        Ok(())
    }

    fn append(&mut self, value: &str) -> Result<(), ParamError> {
        bounded_append(&mut self.output, value, self.limits.max_output_bytes)
    }

    fn encode(&mut self, input: &str) -> Result<(), ParamError> {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for byte in input.bytes() {
            if uri_component_unescaped(byte) {
                self.append(core::str::from_utf8(&[byte]).unwrap())?;
            } else {
                let escaped = [
                    b'%',
                    HEX[usize::from(byte >> 4)],
                    HEX[usize::from(byte & 15)],
                ];
                self.append(core::str::from_utf8(&escaped).unwrap())?;
            }
        }
        Ok(())
    }

    fn add(&mut self, key: &str, value: &Value, depth: usize) -> Result<(), ParamError> {
        if self.pairs >= self.limits.max_pairs {
            return Err(ParamError::PairLimit);
        }
        if self.pairs != 0 {
            self.append("&")?;
        }
        self.encode(key)?;
        self.append("=")?;
        if matches!(value, Value::Null | Value::Undefined) {
            self.visit(depth)?;
        } else {
            let value = self.scalar_string(value, depth)?;
            self.encode(&value)?;
        }
        self.pairs += 1;
        Ok(())
    }

    fn build_params(
        &mut self,
        prefix: &str,
        value: &Value,
        traditional: bool,
        depth: usize,
    ) -> Result<(), ParamError> {
        self.visit(depth)?;
        if prefix.len() > self.limits.max_output_bytes {
            return Err(ParamError::OutputLimit);
        }
        match value {
            Value::Array(items) => {
                self.check_children(items.len())?;
                for (index, item) in items.iter().enumerate() {
                    if traditional || prefix.ends_with("[]") {
                        self.add(prefix, item, depth + 1)?;
                    } else {
                        let index = if matches!(item, Value::Array(_) | Value::Object(_)) {
                            index.to_string()
                        } else {
                            String::new()
                        };
                        let prefix = self.child_prefix(prefix, &index)?;
                        self.build_params(&prefix, item, traditional, depth + 1)?;
                    }
                }
            }
            Value::Object(entries) if !traditional => {
                self.check_children(entries.len())?;
                for (name, item) in value.ordered_entries() {
                    let prefix = self.child_prefix(prefix, name)?;
                    self.build_params(&prefix, item, traditional, depth + 1)?;
                }
            }
            _ => self.add(prefix, value, depth)?,
        }
        Ok(())
    }

    fn child_prefix(&self, prefix: &str, key: &str) -> Result<String, ParamError> {
        let mut child = String::new();
        bounded_append(&mut child, prefix, self.limits.max_output_bytes)?;
        bounded_append(&mut child, "[", self.limits.max_output_bytes)?;
        bounded_append(&mut child, key, self.limits.max_output_bytes)?;
        bounded_append(&mut child, "]", self.limits.max_output_bytes)?;
        Ok(child)
    }

    fn scalar_string(&mut self, value: &Value, depth: usize) -> Result<String, ParamError> {
        let mut output = String::new();
        self.stringify(value, depth, &mut output, false)?;
        Ok(output)
    }

    /// JavaScript String(value), including Array#join's special treatment of
    /// null/undefined entries. The walk stays bounded even in traditional mode.
    fn stringify(
        &mut self,
        value: &Value,
        depth: usize,
        output: &mut String,
        array_item: bool,
    ) -> Result<(), ParamError> {
        self.visit(depth)?;
        let text = match value {
            Value::Undefined if array_item => "",
            Value::Undefined => "undefined",
            Value::Null if array_item => "",
            Value::Null => "null",
            Value::Bool(true) => "true",
            Value::Bool(false) => "false",
            Value::String(value) => value,
            Value::Object(_) => "[object Object]",
            Value::Number(number) => {
                return bounded_append(
                    output,
                    &number_to_string(*number),
                    self.limits.max_output_bytes,
                );
            }
            Value::Array(items) => {
                self.check_children(items.len())?;
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        bounded_append(output, ",", self.limits.max_output_bytes)?;
                    }
                    self.stringify(item, depth + 1, output, true)?;
                }
                return Ok(());
            }
        };
        bounded_append(output, text, self.limits.max_output_bytes)
    }
}

fn bounded_append(output: &mut String, text: &str, limit: usize) -> Result<(), ParamError> {
    if output
        .len()
        .checked_add(text.len())
        .is_none_or(|length| length > limit)
    {
        return Err(ParamError::OutputLimit);
    }
    output.push_str(text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    fn object(entries: &[(&str, Value)]) -> Value {
        Value::Object(
            entries
                .iter()
                .map(|(key, value)| ((*key).into(), value.clone()))
                .collect(),
        )
    }

    fn numbers(items: &[f64]) -> Value {
        Value::Array(items.iter().copied().map(Value::Number).collect())
    }

    fn s(value: &str) -> Value {
        Value::String(value.into())
    }

    #[test]
    fn upstream_simple_null_and_array_cases() {
        let params = object(&[
            ("foo", s("bar")),
            ("baz", Value::Number(42.0)),
            ("quux", s("All your base are belong to us")),
        ]);
        assert_eq!(
            param(&params, false).unwrap(),
            "foo=bar&baz=42&quux=All%20your%20base%20are%20belong%20to%20us"
        );
        let params = object(&[
            ("string", s("foo")),
            ("null", Value::Null),
            ("undefined", Value::Undefined),
        ]);
        assert_eq!(
            param(&params, false).unwrap(),
            "string=foo&null=&undefined="
        );
        let params = object(&[
            ("someName", numbers(&[1.0, 2.0, 3.0])),
            ("regularThing", s("blah")),
        ]);
        assert_eq!(
            param(&params, false).unwrap(),
            "someName%5B%5D=1&someName%5B%5D=2&someName%5B%5D=3&regularThing=blah"
        );
        assert_eq!(
            param(&params, true).unwrap(),
            "someName=1&someName=2&someName=3&regularThing=blah"
        );
        assert_eq!(param(&Value::Undefined, false).unwrap(), "");
        assert_eq!(param(&Value::Null, false).unwrap(), "");
    }

    #[test]
    fn upstream_nested_object_and_array_rules() {
        let params = object(&[(
            "a",
            Value::Array(vec![
                Value::Number(0.0),
                numbers(&[1.0, 2.0]),
                Value::Array(vec![
                    Value::Number(3.0),
                    numbers(&[4.0, 5.0]),
                    numbers(&[6.0]),
                ]),
                object(&[("b", numbers(&[7.0, 8.0]))]),
                Value::Null,
            ]),
        )]);
        assert_eq!(
            param(&params, false).unwrap(),
            "a%5B%5D=0&a%5B1%5D%5B%5D=1&a%5B1%5D%5B%5D=2&a%5B2%5D%5B%5D=3&a%5B2%5D%5B1%5D%5B%5D=4&a%5B2%5D%5B1%5D%5B%5D=5&a%5B2%5D%5B2%5D%5B%5D=6&a%5B3%5D%5Bb%5D%5B%5D=7&a%5B3%5D%5Bb%5D%5B%5D=8&a%5B%5D="
        );
        assert_eq!(
            param(&params, true).unwrap(),
            "a=0&a=1%2C2&a=3%2C4%2C5%2C6&a=%5Bobject%20Object%5D&a="
        );
        let params = object(&[("b[]", Value::Array(vec![numbers(&[1.0, 2.0]), Value::Null]))]);
        assert_eq!(param(&params, false).unwrap(), "b%5B%5D=1%2C2&b%5B%5D=");
    }

    #[test]
    fn keys_follow_javascript_own_property_enumeration() {
        let params = object(&[
            ("z", s("last")),
            ("10", s("ten")),
            ("2", s("two")),
            ("01", s("one")),
            ("2", s("updated")),
        ]);
        assert_eq!(
            param(&params, false).unwrap(),
            "2=updated&10=ten&z=last&01=one"
        );
        assert_eq!(
            param(&object(&[("__proto__", s("safe"))]), false).unwrap(),
            "__proto__=safe"
        );
    }

    #[test]
    fn form_arrays_use_name_value_fields_without_recursive_params() {
        let entries = Value::Array(vec![
            object(&[("name", s("x")), ("value", s("a b"))]),
            object(&[("name", s("x")), ("value", numbers(&[1.0, 2.0]))]),
            object(&[("value", Value::Null)]),
        ]);
        assert_eq!(
            param(&entries, false).unwrap(),
            "x=a%20b&x=1%2C2&undefined="
        );
        assert_eq!(
            param_pairs(&[("x".into(), s("a b")), ("x".into(), Value::Null)]).unwrap(),
            "x=a%20b&x="
        );
        assert_eq!(
            param(&Value::Array(vec![Value::Null]), false),
            Err(ParamError::InvalidFormEntry { index: 0 })
        );
        assert_eq!(
            param(&Value::Bool(true), false),
            Err(ParamError::InvalidRoot)
        );
    }

    #[test]
    fn uri_encoding_and_form_newlines_match_javascript() {
        assert_eq!(
            encode_uri_component("AZaz09-_.!~*'() /+%=&é😀"),
            "AZaz09-_.!~*'()%20%2F%2B%25%3D%26%C3%A9%F0%9F%98%80"
        );
        assert_eq!(
            normalize_form_newlines("a\nb\r\nc\rd\r\r\ne"),
            "a\r\nb\r\nc\rd\r\r\ne"
        );
        assert_eq!(
            param_pairs(&[("number".into(), Value::Number(-0.0))]).unwrap(),
            "number=0"
        );
    }

    #[test]
    fn all_recursion_and_output_paths_are_bounded() {
        let defaults = ParamLimits::default();
        let simple = object(&[("x", s("y"))]);
        assert_eq!(
            param_with_limits(
                &simple,
                false,
                ParamLimits {
                    max_output_bytes: 2,
                    ..defaults
                }
            ),
            Err(ParamError::OutputLimit)
        );
        assert_eq!(
            param_with_limits(
                &simple,
                false,
                ParamLimits {
                    max_pairs: 0,
                    ..defaults
                }
            ),
            Err(ParamError::PairLimit)
        );
        assert_eq!(
            param_with_limits(
                &simple,
                false,
                ParamLimits {
                    max_nodes: 1,
                    ..defaults
                }
            ),
            Err(ParamError::NodeLimit)
        );
        let nested = object(&[("x", Value::Array(vec![numbers(&[1.0])]))]);
        assert_eq!(
            param_with_limits(
                &nested,
                false,
                ParamLimits {
                    max_depth: 1,
                    ..defaults
                }
            ),
            Err(ParamError::DepthLimit)
        );
        assert_eq!(
            param_with_limits(
                &nested,
                true,
                ParamLimits {
                    max_depth: 1,
                    ..defaults
                }
            ),
            Err(ParamError::DepthLimit)
        );
        let pairs: Vec<(String, Value)> = vec![("x".into(), Value::Array(vec![Value::Null; 10]))];
        assert_eq!(
            param_pairs_with_limits(
                &pairs,
                ParamLimits {
                    max_output_bytes: 5,
                    ..defaults
                }
            ),
            Err(ParamError::OutputLimit)
        );
    }
}
