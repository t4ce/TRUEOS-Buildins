//! Native translations of jQuery 4's `src/data.js`, `src/data/Data.js`,
//! `src/data/var/acceptData.js`, and `src/core/camelCase.js`.
//!
//! Owners are host-provided identities rather than JavaScript expando objects.
//! A host must check [`accepts_data`] before admitting a DOM owner. Attribute
//! discovery and private/user cache separation remain the host's responsibility.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::Value;
use crate::value::number_to_string;

/// jQuery 4 camel casing: replace each ASCII `-a` … `-z` pair with `A` … `Z`.
/// In particular, there is no older jQuery `-ms-` prefix special case.
pub fn camel_case(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '-' && chars.peek().is_some_and(char::is_ascii_lowercase) {
            output.push(chars.next().unwrap().to_ascii_uppercase());
        } else {
            output.push(ch);
        }
    }
    output
}

/// Attribute spelling used by jQuery's `dataAttr(elem, key, ...)`.
pub fn data_attribute_name(key: &str) -> String {
    let mut name = String::from("data-");
    for ch in key.chars() {
        if ch.is_ascii_uppercase() {
            name.push('-');
        }
        name.push(ch);
    }
    // Whole-string casing also handles context-sensitive Unicode mappings.
    name.to_lowercase()
}

/// `None` means an ordinary object with no nodeType; zero has the same status.
/// jQuery admits element/document nodes and excludes other numeric node types.
pub fn accepts_data(node_type: Option<u32>) -> bool {
    matches!(node_type, None | Some(0 | 1 | 9))
}

/// The null-prototype property cache used by [`DataStore`]. Raw cache access
/// deliberately does not camel-case keys, just like direct access to `.data()`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DataCache {
    entries: Vec<(String, Value)>,
}

impl DataCache {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries
            .iter_mut()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// Sets an own property without changing its original insertion position.
    pub fn insert(&mut self, key: String, value: Value) -> &Value {
        if let Some(index) = self.entries.iter().position(|(name, _)| *name == key) {
            self.entries[index].1 = value;
            return &self.entries[index].1;
        }
        self.entries.push((key, value));
        &self.entries.last().unwrap().1
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let index = self.entries.iter().position(|(name, _)| name == key)?;
        Some(self.entries.remove(index).1)
    }

    pub fn entries(&self) -> &[(String, Value)] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// `Data.remove` accepts all keys, one possibly whitespace-separated string,
/// or an array whose entries are each literal keys (not split on whitespace).
#[derive(Clone, Copy, Debug)]
pub enum RemoveKeys<'a> {
    All,
    Names(&'a str),
    List(&'a [&'a str]),
}

/// Host-identity equivalent of a single jQuery `Data` instance. Use separate
/// stores for private and user data, as upstream does.
#[derive(Clone, Debug)]
pub struct DataStore<Owner: Ord> {
    caches: BTreeMap<Owner, DataCache>,
}

impl<Owner: Ord> Default for DataStore<Owner> {
    fn default() -> Self {
        Self {
            caches: BTreeMap::new(),
        }
    }
}

impl<Owner: Ord> DataStore<Owner> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reading the whole cache creates it, matching `Data.get(owner)`.
    pub fn cache(&mut self, owner: Owner) -> &mut DataCache {
        self.caches.entry(owner).or_default()
    }

    pub fn cache_if_present(&self, owner: &Owner) -> Option<&DataCache> {
        self.caches.get(owner)
    }

    /// Reading one key does not create a cache.
    pub fn get(&self, owner: &Owner, key: &str) -> Option<&Value> {
        self.caches.get(owner)?.get(&camel_case(key))
    }

    /// Low-level `Data.set`: unlike the public `.data(key, undefined)` overload,
    /// this operation can store an own property whose value is `Undefined`.
    pub fn set(&mut self, owner: Owner, key: &str, value: Value) -> &Value {
        self.cache(owner).insert(camel_case(key), value)
    }

    /// Extends the existing cache rather than replacing it.
    pub fn set_many(
        &mut self,
        owner: Owner,
        values: impl IntoIterator<Item = (String, Value)>,
    ) -> &mut DataCache {
        let cache = self.cache(owner);
        for (key, value) in values {
            cache.insert(camel_case(&key), value);
        }
        cache
    }

    /// String-key branch of upstream `Data.access`. An absent or undefined
    /// value reads a nonempty key; an empty key takes the source's write path.
    /// This overloaded operation returns an owned snapshot; use `get` or
    /// `cache` when the caller needs borrowed cache access.
    pub fn access(&mut self, owner: Owner, key: &str, value: Option<Value>) -> Value {
        if !key.is_empty() && matches!(value, None | Some(Value::Undefined)) {
            self.get(&owner, key).cloned().unwrap_or(Value::Undefined)
        } else {
            let value = value.unwrap_or(Value::Undefined);
            let result = if matches!(value, Value::Undefined) {
                Value::String(key.to_string())
            } else {
                value.clone()
            };
            self.set(owner, key, value);
            result
        }
    }

    pub fn remove(&mut self, owner: &Owner, keys: RemoveKeys<'_>) {
        let Some(cache) = self.caches.get_mut(owner) else {
            return;
        };
        match keys {
            RemoveKeys::All => cache.clear(),
            RemoveKeys::List(names) => {
                for name in names {
                    cache.remove(&camel_case(name));
                }
            }
            RemoveKeys::Names(names) => {
                let names = camel_case(names);
                // An exact property containing whitespace takes precedence.
                if cache.get(&names).is_some() {
                    cache.remove(&names);
                } else {
                    for name in names
                        .split(is_html_whitespace)
                        .filter(|name| !name.is_empty())
                    {
                        cache.remove(name);
                    }
                }
            }
        }
        if cache.is_empty() {
            self.caches.remove(owner);
        }
    }

    pub fn has_data(&self, owner: &Owner) -> bool {
        self.caches
            .get(owner)
            .is_some_and(|cache| !cache.is_empty())
    }
}

fn is_html_whitespace(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\r' | '\n' | '\u{000c}')
}

/// Native parser resource limits. These bounds are explicit host policy, not
/// additional JavaScript/JSON syntax rules. Sizes count UTF-8 input bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonLimits {
    pub max_input_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
}

impl Default for JsonLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 1_048_576,
            max_depth: 128,
            max_nodes: 100_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonError {
    Syntax {
        offset: usize,
    },
    /// Valid JavaScript strings can contain unpaired UTF-16 surrogates; Rust
    /// UTF-8 strings cannot. They are reported rather than changed or accepted
    /// as a different value.
    UnpairedSurrogate {
        offset: usize,
    },
    InputLimit,
    DepthLimit,
    NodeLimit,
}

pub type DataParseError = JsonError;

/// jQuery `getData` plus `dataAttr`'s invalid-JSON fallback. Ordinary invalid
/// JSON stays a string; resource and representation limits are errors.
pub fn parse_data_attribute(input: &str) -> Result<Value, DataParseError> {
    parse_data_attribute_with_limits(input, JsonLimits::default())
}

pub fn parse_data_attribute_with_limits(
    input: &str,
    limits: JsonLimits,
) -> Result<Value, DataParseError> {
    if input.len() > limits.max_input_bytes {
        return Err(JsonError::InputLimit);
    }
    match input {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        "null" => return Ok(Value::Null),
        _ => {}
    }

    // Any spelling accepted here must survive JavaScript Number->String
    // unchanged. Nondecimal/whitespace forms can never pass that comparison.
    let number = match input {
        "Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        "NaN" => Some(f64::NAN),
        _ => input.parse::<f64>().ok(),
    };
    if let Some(number) = number {
        if number_to_string(number) == input {
            return Ok(Value::Number(number));
        }
    }

    if (input.starts_with('{') && input.ends_with('}'))
        || (input.starts_with('[') && input.ends_with(']'))
    {
        return match parse_json_with_limits(input, limits) {
            Err(JsonError::Syntax { .. }) => Ok(Value::String(input.to_string())),
            result => result,
        };
    }
    Ok(Value::String(input.to_string()))
}

/// Dependency-free JSON.parse equivalent over [`Value`]'s representable value
/// domain. There is no reviver or prototype lookup, and duplicate own keys
/// replace their value without changing the first insertion position.
pub fn parse_json(input: &str) -> Result<Value, JsonError> {
    parse_json_with_limits(input, JsonLimits::default())
}

pub fn parse_json_with_limits(input: &str, limits: JsonLimits) -> Result<Value, JsonError> {
    if input.len() > limits.max_input_bytes {
        return Err(JsonError::InputLimit);
    }
    let mut parser = JsonParser {
        input,
        offset: 0,
        nodes: 0,
        limits,
        unpaired_surrogate: None,
    };
    let value = parser.value(0)?;
    parser.whitespace();
    if parser.offset != input.len() {
        return Err(parser.syntax());
    }
    if let Some(offset) = parser.unpaired_surrogate {
        return Err(JsonError::UnpairedSurrogate { offset });
    }
    Ok(value)
}

struct JsonParser<'a> {
    input: &'a str,
    offset: usize,
    nodes: usize,
    limits: JsonLimits,
    unpaired_surrogate: Option<usize>,
}

impl JsonParser<'_> {
    fn syntax(&self) -> JsonError {
        JsonError::Syntax {
            offset: self.offset,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.offset).copied()
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.offset += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, JsonError> {
        if depth > self.limits.max_depth {
            return Err(JsonError::DepthLimit);
        }
        if self.nodes >= self.limits.max_nodes {
            return Err(JsonError::NodeLimit);
        }
        self.nodes += 1;
        self.whitespace();
        match self.peek() {
            Some(b'n') => self.literal("null", Value::Null),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => self.array(depth),
            Some(b'{') => self.object(depth),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.syntax()),
        }
    }

    fn literal(&mut self, literal: &str, value: Value) -> Result<Value, JsonError> {
        if !self.input[self.offset..].starts_with(literal) {
            return Err(self.syntax());
        }
        self.offset += literal.len();
        Ok(value)
    }

    fn array(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.offset += 1;
        self.whitespace();
        let mut values = Vec::new();
        if self.peek() == Some(b']') {
            self.offset += 1;
            return Ok(Value::Array(values));
        }
        loop {
            values.push(self.value(depth + 1)?);
            self.whitespace();
            match self.peek() {
                Some(b']') => {
                    self.offset += 1;
                    return Ok(Value::Array(values));
                }
                Some(b',') => self.offset += 1,
                _ => return Err(self.syntax()),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.offset += 1;
        self.whitespace();
        let mut values: Vec<(String, Value)> = Vec::new();
        let mut positions: BTreeMap<String, usize> = BTreeMap::new();
        if self.peek() == Some(b'}') {
            self.offset += 1;
            return Ok(Value::Object(values));
        }
        loop {
            self.whitespace();
            if self.peek() != Some(b'"') {
                return Err(self.syntax());
            }
            let name = self.string()?;
            self.whitespace();
            if self.peek() != Some(b':') {
                return Err(self.syntax());
            }
            self.offset += 1;
            let value = self.value(depth + 1)?;
            if let Some(&index) = positions.get(&name) {
                values[index].1 = value;
            } else {
                positions.insert(name.clone(), values.len());
                values.push((name, value));
            }
            self.whitespace();
            match self.peek() {
                Some(b'}') => {
                    self.offset += 1;
                    return Ok(Value::Object(values));
                }
                Some(b',') => self.offset += 1,
                _ => return Err(self.syntax()),
            }
        }
    }

    fn number(&mut self) -> Result<Value, JsonError> {
        let start = self.offset;
        if self.peek() == Some(b'-') {
            self.offset += 1;
        }
        match self.peek() {
            Some(b'0') => self.offset += 1,
            Some(b'1'..=b'9') => {
                self.offset += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.offset += 1;
                }
            }
            _ => return Err(self.syntax()),
        }
        if self.peek() == Some(b'.') {
            self.offset += 1;
            let digits = self.offset;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.offset += 1;
            }
            if self.offset == digits {
                return Err(self.syntax());
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.offset += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            let digits = self.offset;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.offset += 1;
            }
            if self.offset == digits {
                return Err(self.syntax());
            }
        }
        self.input[start..self.offset]
            .parse::<f64>()
            .map(Value::Number)
            .map_err(|_| self.syntax())
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.offset += 1; // Opening double quote checked by the caller.
        let mut result = String::new();
        let mut start = self.offset;
        loop {
            match self.peek() {
                None | Some(0..=0x1f) => return Err(self.syntax()),
                Some(b'"') => {
                    result.push_str(&self.input[start..self.offset]);
                    self.offset += 1;
                    return Ok(result);
                }
                Some(b'\\') => {
                    result.push_str(&self.input[start..self.offset]);
                    self.offset += 1;
                    match self.peek() {
                        Some(b'"') => result.push('"'),
                        Some(b'\\') => result.push('\\'),
                        Some(b'/') => result.push('/'),
                        Some(b'b') => result.push('\u{0008}'),
                        Some(b'f') => result.push('\u{000c}'),
                        Some(b'n') => result.push('\n'),
                        Some(b'r') => result.push('\r'),
                        Some(b't') => result.push('\t'),
                        Some(b'u') => {
                            self.offset += 1;
                            let escape = self.offset;
                            let high = self.hex_quad()?;
                            let scalar = if (0xd800..=0xdbff).contains(&high) {
                                let next_escape = self.offset;
                                let low = if self.input[self.offset..].starts_with("\\u") {
                                    self.offset += 2;
                                    Some(self.hex_quad()?)
                                } else {
                                    None
                                };
                                if let Some(low @ 0xdc00..=0xdfff) = low {
                                    0x10000
                                        + ((u32::from(high) - 0xd800) << 10)
                                        + (u32::from(low) - 0xdc00)
                                } else {
                                    self.offset = next_escape;
                                    self.unpaired_surrogate.get_or_insert(escape);
                                    // Internal placeholder only: the complete
                                    // parse must validate syntax before this
                                    // representation error can be returned.
                                    0xfffd
                                }
                            } else if (0xdc00..=0xdfff).contains(&high) {
                                self.unpaired_surrogate.get_or_insert(escape);
                                0xfffd
                            } else {
                                u32::from(high)
                            };
                            result.push(char::from_u32(scalar).unwrap());
                            start = self.offset;
                            continue;
                        }
                        _ => return Err(self.syntax()),
                    }
                    self.offset += 1;
                    start = self.offset;
                }
                // UTF-8 continuation bytes cannot match quote/backslash/control;
                // slicing happens only at an ASCII boundary.
                Some(_) => self.offset += 1,
            }
        }
    }

    fn hex_quad(&mut self) -> Result<u16, JsonError> {
        let mut value = 0;
        for _ in 0..4 {
            let digit = match self.peek() {
                Some(b'0'..=b'9') => self.peek().unwrap() - b'0',
                Some(b'a'..=b'f') => self.peek().unwrap() - b'a' + 10,
                Some(b'A'..=b'F') => self.peek().unwrap() - b'A' + 10,
                _ => return Err(self.syntax()),
            };
            value = (value << 4) | u16::from(digit);
            self.offset += 1;
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn camel_case_is_the_jquery_four_algorithm() {
        for (input, expected) in [
            ("foo-bar", "fooBar"),
            ("foo-42", "foo-42"),
            ("nested--double", "nested-Double"),
            ("-ms-grid", "MsGrid"),
            ("a-Ä-a", "a-ÄA"),
            ("a-A", "a-A"),
            ("foo---", "foo---"),
        ] {
            assert_eq!(camel_case(input), expected);
        }
        assert_eq!(data_attribute_name("fooBar"), "data-foo-bar");
        assert_eq!(data_attribute_name("foo-42"), "data-foo-42");
    }

    #[test]
    fn data_store_preserves_cache_and_undefined_semantics() {
        let mut data = DataStore::<u32>::new();
        assert_eq!(data.get(&1, "foo"), None);
        assert!(data.cache_if_present(&1).is_none());
        data.set(1, "foo-bar", Value::String("first".into()));
        data.access(1, "foo-bar", Some(Value::Undefined));
        assert_eq!(data.get(&1, "fooBar"), Some(&Value::String("first".into())));
        data.set_many(1, vec![("second-key".into(), Value::Null)]);
        assert_eq!(data.cache(1).len(), 2);
        data.set(1, "fooBar", Value::Bool(false));
        assert_eq!(data.cache(1).entries()[0].0, "fooBar");
        data.cache(1).insert("raw-key".into(), Value::Undefined);
        assert!(data.has_data(&1));
        assert_eq!(data.get(&1, "raw-key"), None);
        assert_eq!(data.cache(1).get("raw-key"), Some(&Value::Undefined));
        assert_eq!(data.access(2, "", None), Value::String(String::new()));
        assert_eq!(data.get(&2, ""), Some(&Value::Undefined));
    }

    #[test]
    fn removal_prefers_exact_key_and_splits_only_html_whitespace() {
        let mut data = DataStore::<u32>::new();
        for name in ["a", "b", "a b", "x-y", "x\u{a0}y"] {
            data.set(1, name, Value::Bool(true));
        }
        data.remove(&1, RemoveKeys::Names("a b"));
        assert!(data.get(&1, "a b").is_none());
        assert!(data.get(&1, "a").is_some());
        data.remove(&1, RemoveKeys::Names("a\t b\r\n x-y\u{c}"));
        assert!(data.get(&1, "a").is_none());
        assert!(data.get(&1, "xY").is_none());
        assert!(data.get(&1, "x\u{a0}y").is_some());
        data.remove(&1, RemoveKeys::List(&["x\u{a0}y"]));
        assert!(!data.has_data(&1));
        assert!(data.cache_if_present(&1).is_none());
    }

    #[test]
    fn attributes_follow_upstream_data_unit_cases() {
        for (input, expected) in [
            ("true", Value::Bool(true)),
            ("false", Value::Bool(false)),
            ("null", Value::Null),
            ("5", Value::Number(5.0)),
            ("5.5", Value::Number(5.5)),
            ("-5", Value::Number(-5.0)),
            ("Infinity", Value::Number(f64::INFINITY)),
        ] {
            assert_eq!(parse_data_attribute(input), Ok(expected));
        }
        for input in [
            "5.5E3",
            "5.574E9",
            "0x42",
            "5..5",
            "-.",
            "123456789123456789123456789",
            "{123}",
            "[abc]",
            " {}",
            "[] ",
            "",
            " ",
            "-0",
            "01",
            "1.0",
            "1e3",
            "test",
        ] {
            assert_eq!(parse_data_attribute(input), Ok(Value::String(input.into())));
        }
        assert!(matches!(parse_data_attribute("NaN"), Ok(Value::Number(n)) if n.is_nan()));
    }

    #[test]
    fn json_supports_unicode_and_duplicate_keys_without_prototypes() {
        assert_eq!(
            parse_json(r#"{"x":[true,null,-1.25e2],"emoji":"\uD83D\uDE00","x":3,"__proto__":{}}"#),
            Ok(Value::Object(vec![
                ("x".into(), Value::Number(3.0)),
                ("emoji".into(), Value::String("😀".into())),
                ("__proto__".into(), Value::Object(vec![])),
            ]))
        );
        assert_eq!(
            parse_json(" \t\r\n[\"ä\",\"\\b\\f\\n\\r\\t\\/\\\\\\\"\"] "),
            Ok(Value::Array(vec![
                Value::String("ä".into()),
                Value::String("\u{8}\u{c}\n\r\t/\\\"".into())
            ]))
        );
        assert!(matches!(parse_json("-0"), Ok(Value::Number(n)) if n.is_sign_negative()));
        assert!(matches!(parse_json("1e400"), Ok(Value::Number(n)) if n.is_infinite()));
    }

    #[test]
    fn json_invalid_syntax_and_unrepresentable_strings_are_distinct() {
        for input in [
            "",
            "01",
            "-",
            "1.",
            ".1",
            "[1,]",
            "{\"a\":1,}",
            "[true false]",
            "[NaN]",
            "\"\\q\"",
            "\"\n\"",
            "[1]x",
        ] {
            assert!(
                matches!(parse_json(input), Err(JsonError::Syntax { .. })),
                "{input}"
            );
        }
        for input in [r#""\uD800""#, r#""\uDC00""#, r#"["\uD800x"]"#] {
            assert!(matches!(
                parse_json(input),
                Err(JsonError::UnpairedSurrogate { .. })
            ));
        }
        assert!(matches!(
            parse_data_attribute(r#"["\uD800"]"#),
            Err(JsonError::UnpairedSurrogate { .. })
        ));
        assert!(matches!(
            parse_json(r#"["\uD800"x]"#),
            Err(JsonError::Syntax { .. })
        ));
        assert_eq!(
            parse_data_attribute(r#"["\uD800"x]"#),
            Ok(Value::String(r#"["\uD800"x]"#.into()))
        );
    }

    #[test]
    fn limits_are_explicit_and_never_become_attribute_strings() {
        let limits = JsonLimits {
            max_input_bytes: 10,
            max_depth: 1,
            max_nodes: 2,
        };
        assert_eq!(
            parse_json_with_limits("[[[]]]", limits),
            Err(JsonError::DepthLimit)
        );
        assert_eq!(
            parse_data_attribute_with_limits("[1,2]", limits),
            Err(JsonError::NodeLimit)
        );
        assert_eq!(
            parse_data_attribute_with_limits("01234567890", limits),
            Err(JsonError::InputLimit)
        );
        assert!(accepts_data(None));
        assert!(accepts_data(Some(9)));
        assert!(!accepts_data(Some(3)));
        assert!(!accepts_data(Some(11)));
    }
}
