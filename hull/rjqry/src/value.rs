//! Owned data values for portable jQuery algorithms. Executable JS functions,
//! accessors, prototypes and cyclic object identity belong to a future binding.
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}
impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.into())
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}
impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}
impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Number(value)
    }
}
impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Self::Number(value.into())
    }
}
impl Value {
    pub fn js_string(&self) -> String {
        match self {
            Self::Undefined => "undefined".into(),
            Self::Null => "null".into(),
            Self::Bool(true) => "true".into(),
            Self::Bool(false) => "false".into(),
            Self::Number(number) => number_to_string(*number),
            Self::String(text) => text.clone(),
            Self::Object(_) => "[object Object]".into(),
            Self::Array(values) => values
                .iter()
                .map(|value| match value {
                    Self::Undefined | Self::Null => String::new(),
                    _ => value.js_string(),
                })
                .collect::<Vec<_>>()
                .join(","),
        }
    }
    pub fn truthy(&self) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Bool(value) => *value,
            Self::Number(value) => *value != 0.0 && !value.is_nan(),
            Self::String(value) => !value.is_empty(),
            _ => true,
        }
    }
    pub fn object_get(&self, name: &str) -> Option<&Value> {
        let Self::Object(entries) = self else {
            return None;
        };
        entries
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }
    pub fn set_property(&mut self, name: String, value: Value) {
        let Self::Object(entries) = self else {
            *self = Self::Object(Vec::new());
            return self.set_property(name, value);
        };
        if let Some(index) = entries.iter().position(|(key, _)| key == &name) {
            entries[index].1 = value;
            let mut current = index + 1;
            while current < entries.len() {
                if entries[current].0 == name {
                    entries.remove(current);
                } else {
                    current += 1;
                }
            }
        } else {
            entries.push((name, value));
        }
    }
    /// Own enumerable property order for plain data objects: array-index names
    /// first, followed by other names in their first insertion position.
    pub fn ordered_entries(&self) -> Vec<(&str, &Value)> {
        let Self::Object(entries) = self else {
            return Vec::new();
        };
        let mut result = Vec::new();
        for (key, _) in entries {
            if !result.iter().any(|(previous, _)| *previous == key.as_str()) {
                result.push((key.as_str(), self.object_get(key).unwrap()));
            }
        }
        result.sort_by(
            |(left, _), (right, _)| match (array_index(left), array_index(right)) {
                (Some(a), Some(b)) => a.cmp(&b),
                (Some(_), None) => core::cmp::Ordering::Less,
                (None, Some(_)) => core::cmp::Ordering::Greater,
                (None, None) => core::cmp::Ordering::Equal,
            },
        );
        result
    }
}
fn array_index(name: &str) -> Option<u32> {
    let value = name.parse::<u32>().ok()?;
    (value != u32::MAX && value.to_string() == name).then_some(value)
}

/// ECMAScript formatting thresholds around Rust's shortest-roundtrip digits.
pub fn number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value == f64::INFINITY {
        return "Infinity".into();
    }
    if value == f64::NEG_INFINITY {
        return "-Infinity".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    let raw = value.to_string();
    let (sign, unsigned) = raw
        .strip_prefix('-')
        .map_or(("", raw.as_str()), |s| ("-", s));
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, 0), |(m, e)| (m, e.parse::<i32>().unwrap()));
    let left = mantissa.find('.').unwrap_or(mantissa.len()) as i32;
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let leading = digits.bytes().take_while(|&c| c == b'0').count();
    let digits = digits[leading..].trim_end_matches('0');
    let power = exponent + left - leading as i32 - 1;
    if !(-6..21).contains(&power) {
        let fraction = if digits.len() > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        return format!(
            "{sign}{}{fraction}e{}{power}",
            &digits[..1],
            if power >= 0 { "+" } else { "" }
        );
    }
    if power < 0 {
        return format!("{sign}0.{}{}", "0".repeat((-power - 1) as usize), digits);
    }
    let point = power as usize + 1;
    if digits.len() <= point {
        format!("{sign}{digits}{}", "0".repeat(point - digits.len()))
    } else {
        format!("{sign}{}.{}", &digits[..point], &digits[point..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    #[test]
    fn numbers_follow_javascript_thresholds_and_special_values() {
        for (number, expected) in [
            (0.0, "0"),
            (-0.0, "0"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (-12.5, "-12.5"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
        ] {
            assert_eq!(number_to_string(number), expected);
        }
    }
    #[test]
    fn object_order_and_array_stringification_preserve_data_semantics() {
        let object = Value::Object(vec![
            ("b".into(), Value::Null),
            ("10".into(), Value::Null),
            ("2".into(), Value::Null),
            ("b".into(), Value::Bool(true)),
            ("01".into(), Value::Null),
        ]);
        assert_eq!(
            object
                .ordered_entries()
                .iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            ["2", "10", "b", "01"]
        );
        assert_eq!(object.object_get("b"), Some(&Value::Bool(true)));
        assert_eq!(
            Value::Array(vec![
                Value::Null,
                Value::Array(vec![Value::Number(2.0), Value::Number(3.0)]),
                Value::Undefined
            ])
            .js_string(),
            ",2,3,"
        );
    }
}
