//! Portable prefilter registration, settings merge and response conversion from
//! `jquery-main/src/ajax.js`. No request, transport, URL origin, browser or I/O
//! is installed. A host supplies typed callbacks and interprets their errors.
use crate::{
    Value,
    utilities::{ExtendError, extend, html_space},
};
use alloc::{collections::VecDeque, format, rc::Rc, string::String, vec::Vec};

/// Registration order and `+type` prepend semantics of ajaxPrefilter/Transport.
#[derive(Clone, Debug)]
pub struct Registry<H> {
    entries: Vec<(String, Vec<H>)>,
}
impl<H> Default for Registry<H> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}
impl<H: Clone> Registry<H> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add(&mut self, expression: Option<&str>, hook: H) {
        for token in expression
            .unwrap_or("*")
            .to_lowercase()
            .split(html_space)
            .filter(|s| !s.is_empty())
        {
            let (first, name) = token.strip_prefix('+').map_or((false, token), |s| {
                (true, if s.is_empty() { "*" } else { s })
            });
            let index = self
                .entries
                .iter()
                .position(|(key, _)| key == name)
                .unwrap_or_else(|| {
                    self.entries.push((name.into(), Vec::new()));
                    self.entries.len() - 1
                });
            if first {
                self.entries[index].1.insert(0, hook.clone());
            } else {
                self.entries[index].1.push(hook.clone());
            }
        }
    }
    pub fn hooks(&self, data_type: &str) -> &[H] {
        self.entries
            .iter()
            .find(|(key, _)| key == data_type)
            .map_or(&[], |(_, hooks)| hooks.as_slice())
    }
    /// The caller carries original options/jqXHR in its callback context.
    /// An explicit redirect limit bounds native recursion; ordinary source
    /// behavior is preserved until this supplied resource bound is reached.
    pub fn inspect_prefilters(
        &self,
        data_types: &mut Vec<String>,
        limit: usize,
        mut inspect: impl FnMut(&H, &mut Vec<String>) -> Option<String>,
    ) -> Result<(), InspectError> {
        fn visit<H: Clone, F: FnMut(&H, &mut Vec<String>) -> Option<String>>(
            registry: &Registry<H>,
            kind: String,
            types: &mut Vec<String>,
            seen: &mut Vec<String>,
            limit: usize,
            callback: &mut F,
        ) -> Result<(), InspectError> {
            if seen.len() >= limit {
                return Err(InspectError::RedirectLimit);
            }
            seen.push(kind.clone());
            for hook in registry.hooks(&kind) {
                if let Some(next) = callback(hook, types) {
                    if !seen.contains(&next) {
                        types.insert(0, next.clone());
                        visit(registry, next, types, seen, limit, callback)?;
                        break;
                    }
                }
            }
            Ok(())
        }
        let mut seen = Vec::new();
        let first = data_types.first().cloned().unwrap_or_default();
        visit(self, first, data_types, &mut seen, limit, &mut inspect)?;
        if !seen.iter().any(|s| s == "*") {
            visit(self, "*".into(), data_types, &mut seen, limit, &mut inspect)?;
        }
        Ok(())
    }
    pub fn inspect_transports<T>(
        &self,
        data_type: &str,
        mut inspect: impl FnMut(&H) -> Option<T>,
    ) -> Option<T> {
        for hook in self.hooks(data_type) {
            if let Some(transport) = inspect(hook) {
                return Some(transport);
            }
        }
        if data_type != "*" {
            for hook in self.hooks("*") {
                if let Some(transport) = inspect(hook) {
                    return Some(transport);
                }
            }
        }
        None
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectError {
    RedirectLimit,
}

/// jQuery.ajaxSettings.flatOptions defaults to `url` and `context`. The caller
/// can provide custom names, as the upstream settings object allows.
pub fn extend_settings(
    target: &mut Value,
    source: &Value,
    flat_options: &[&str],
) -> Result<(), ExtendError> {
    let mut deep = Value::Object(Vec::new());
    for (key, value) in source.ordered_entries() {
        if *value == Value::Undefined {
            continue;
        }
        if flat_options.contains(&key) {
            target.set_property(key.into(), value.clone());
        } else {
            deep.set_property(key.into(), value.clone());
        }
    }
    extend(target, &[deep], true)
}

#[derive(Clone)]
pub enum Converter {
    Identity,
    Function(Rc<dyn Fn(Value) -> Result<Value, Value>>),
}
impl Converter {
    pub fn new(convert: impl Fn(Value) -> Result<Value, Value> + 'static) -> Self {
        Self::Function(Rc::new(convert))
    }
}
pub type DataFilter = Rc<dyn Fn(Value, &str) -> Result<Value, Value>>;
#[derive(Clone, Default)]
pub struct ConversionOptions {
    pub data_types: Vec<String>,
    pub data_type: String,
    pub converters: Vec<(String, Converter)>,
    pub response_fields: Vec<(String, String)>,
    pub data_filter: Option<DataFilter>,
    pub throws: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversionState {
    Success,
    ParserError,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ConvertedResponse {
    pub state: ConversionState,
    pub data: Option<Value>,
    pub error: Option<Value>,
    pub response_fields: Vec<(String, Value)>,
}
/// A thrown converter/dataFilter value, together with the jqXHR response fields
/// already assigned before that abrupt completion in the upstream algorithm.
#[derive(Clone, Debug, PartialEq)]
pub struct ConversionFailure {
    pub error: Value,
    pub response_fields: Vec<(String, Value)>,
}
fn converter<'a>(converters: &'a [(String, Converter)], name: &str) -> Option<&'a Converter> {
    converters
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, c)| c)
}
/// Source ajaxHandleResponses policy. MIME matching is provided in source order
/// by the host (actual RegExp execution stays in the JS binding). The response
/// type inserted here becomes the first stage of `convert_response`.
pub fn select_response(
    options: &mut ConversionOptions,
    responses: &[(String, Value)],
    content_type: Option<&str>,
    mime_override: Option<&str>,
    content_matches: &[(String, bool)],
) -> Option<Value> {
    let mut auto = false;
    while options.data_types.first().is_some_and(|s| s == "*") {
        options.data_types.remove(0);
        auto = true;
    }
    if auto
        && mime_override
            .filter(|s| !s.is_empty())
            .or(content_type)
            .is_some_and(|s| !s.is_empty())
    {
        if let Some((kind, _)) = content_matches.iter().find(|(_, matched)| *matched) {
            options.data_types.insert(0, kind.clone());
        }
    }
    let desired = options.data_types.first();
    let exact = desired.and_then(|kind| responses.iter().find(|(key, _)| key == kind));
    let mut first: Option<&(String, Value)> = None;
    let convertible = responses.iter().find(|entry| {
        let (key, _) = *entry;
        if desired.is_none_or(|kind| kind.is_empty())
            || converter(&options.converters, &format!("{key} {}", desired.unwrap())).is_some()
        {
            return true;
        }
        // Upstream's firstDataType uses a truthiness test, so an empty key
        // does not prevent a later nonempty key becoming the fallback.
        if first.is_none_or(|(name, _)| name.is_empty()) {
            first = Some(*entry);
        }
        false
    });
    let selected = exact.or(convertible).or(first)?;
    if selected.0.is_empty() {
        return None;
    }
    if options.data_types.first() != Some(&selected.0) {
        options.data_types.insert(0, selected.0.clone());
    }
    Some(selected.1.clone())
}
/// Native Result errors stand for thrown JS values. dataFilter exceptions always
/// propagate; converter exceptions produce parsererror unless `throws` is set.
/// Both normal completion and propagated errors retain assigned response fields.
/// Rust panics are never caught. Responses/contexts are owned data values.
pub fn convert_response(
    options: &ConversionOptions,
    mut response: Value,
    is_success: bool,
) -> Result<ConvertedResponse, ConversionFailure> {
    let mut converters: Vec<(String, Converter)> = Vec::new();
    if options.data_types.len() > 1 {
        for (key, value) in &options.converters {
            let key = key.to_lowercase();
            if let Some((_, previous)) = converters.iter_mut().find(|(name, _)| name == &key) {
                *previous = value.clone();
            } else {
                converters.push((key, value.clone()));
            }
        }
    }
    let mut types = VecDeque::from(options.data_types.clone());
    let mut current = types.pop_front();
    let mut previous: Option<String> = None;
    let mut fields = Vec::new();
    while let Some(kind) = current.take().filter(|s| !s.is_empty()) {
        if let Some((_, field)) = options
            .response_fields
            .iter()
            .rev()
            .find(|(key, _)| key == &kind)
            .filter(|(_, field)| !field.is_empty())
        {
            if let Some((_, value)) = fields.iter_mut().find(|(key, _)| key == field) {
                *value = response.clone();
            } else {
                fields.push((field.clone(), response.clone()));
            }
        }
        if previous.is_none() && is_success {
            if let Some(filter) = &options.data_filter {
                response = match filter(response, &options.data_type) {
                    Ok(response) => response,
                    Err(error) => {
                        return Err(ConversionFailure {
                            error,
                            response_fields: fields,
                        });
                    }
                };
            }
        }
        previous = Some(kind.clone());
        current = types.pop_front();
        if let Some(next) = current.as_mut().filter(|next| !next.is_empty()) {
            if next == "*" {
                *next = kind;
                continue;
            }
            if kind == "*" || kind == *next {
                continue;
            }
            let mut selected = converter(&converters, &format!("{kind} {next}"))
                .or_else(|| converter(&converters, &format!("* {next}")))
                .cloned();
            if selected.is_none() {
                for (key, second) in &converters {
                    let parts = key.split(' ').collect::<Vec<_>>();
                    if parts.get(1).copied() != Some(next.as_str()) {
                        continue;
                    }
                    let first = converter(&converters, &format!("{kind} {}", parts[0]))
                        .or_else(|| converter(&converters, &format!("* {}", parts[0])));
                    if let Some(first) = first {
                        selected = Some(first.clone());
                        if matches!(first, Converter::Identity) {
                            selected = Some(second.clone());
                        } else if !matches!(second, Converter::Identity) {
                            let output = next.clone();
                            *next = parts[0].into();
                            types.push_front(output);
                        }
                        break;
                    }
                }
            }
            let result = match selected {
                Some(Converter::Identity) => continue,
                Some(Converter::Function(ref callback)) => callback(response),
                None => Err(Value::String(format!(
                    "No conversion from {kind} to {next}"
                ))),
            };
            match result {
                Ok(value) => response = value,
                Err(error) => {
                    if selected.is_some() && options.throws {
                        return Err(ConversionFailure {
                            error,
                            response_fields: fields,
                        });
                    }
                    return Ok(ConvertedResponse {
                        state: ConversionState::ParserError,
                        data: None,
                        error: Some(error),
                        response_fields: fields,
                    });
                }
            }
        }
    }
    Ok(ConvertedResponse {
        state: ConversionState::Success,
        data: Some(response),
        error: None,
        response_fields: fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::ToString, vec};
    #[test]
    fn hook_order_redirect_cycle_guard_and_transport_fallback() {
        let mut registry = Registry::new();
        registry.add(Some("text"), 1);
        registry.add(Some("+text json"), 2);
        registry.add(None, 3);
        assert_eq!(registry.hooks("text"), [2, 1]);
        let mut types = vec!["text".into()];
        let mut trace = Vec::new();
        registry
            .inspect_prefilters(&mut types, 10, |hook, _| {
                trace.push(*hook);
                match hook {
                    2 => Some("json".into()),
                    _ => None,
                }
            })
            .unwrap();
        assert_eq!(trace, [2, 2, 3]);
        assert_eq!(types, ["json", "text"]);
        assert_eq!(
            registry.inspect_transports("unknown", |h| (*h == 3).then_some(99)),
            Some(99)
        );
        assert_eq!(
            registry.inspect_prefilters(&mut types, 0, |_, _| None),
            Err(InspectError::RedirectLimit)
        );
    }
    #[test]
    fn settings_flat_options_replace_while_other_options_merge() {
        let mut target = Value::Object(vec![
            (
                "context".into(),
                Value::Object(vec![("a".into(), 1.into())]),
            ),
            (
                "headers".into(),
                Value::Object(vec![("a".into(), 1.into())]),
            ),
        ]);
        let source = Value::Object(vec![
            (
                "context".into(),
                Value::Object(vec![("b".into(), 2.into())]),
            ),
            (
                "headers".into(),
                Value::Object(vec![("b".into(), 2.into())]),
            ),
        ]);
        extend_settings(&mut target, &source, &["url", "context"]).unwrap();
        assert!(
            target
                .object_get("context")
                .unwrap()
                .object_get("a")
                .is_none()
        );
        assert_eq!(
            target.object_get("headers").unwrap().object_get("a"),
            Some(&1.into())
        );
    }
    fn append(label: &'static str) -> Converter {
        Converter::new(move |value| Ok(Value::String(value.js_string() + label)))
    }
    #[test]
    fn conversion_intermediates_identity_wildcards_and_response_fields() {
        let options = ConversionOptions {
            data_types: vec!["text".into(), "json".into()],
            converters: vec![
                ("TEXT MID".into(), append("A")),
                ("mid json".into(), append("B")),
            ],
            response_fields: vec![
                ("text".into(), "responseText".into()),
                ("json".into(), "responseJSON".into()),
            ],
            ..Default::default()
        };
        let result = convert_response(&options, "x".into(), true).unwrap();
        assert_eq!(result.data, Some("xAB".into()));
        assert_eq!(
            result.response_fields,
            vec![
                ("responseText".into(), "x".into()),
                ("responseJSON".into(), "xAB".into())
            ]
        );
        let options = ConversionOptions {
            converters: vec![
                ("* mid".into(), Converter::Identity),
                ("mid json".into(), append("B")),
            ],
            ..options
        };
        assert_eq!(
            convert_response(&options, "x".into(), false).unwrap().data,
            Some("xB".into())
        );
    }
    #[test]
    fn errors_match_parsererror_and_throws_boundary() {
        let mut options = ConversionOptions {
            data_types: vec!["text".into(), "json".into()],
            response_fields: vec![("text".into(), "responseText".into())],
            ..Default::default()
        };
        let result = convert_response(&options, "x".into(), true).unwrap();
        assert_eq!(result.state, ConversionState::ParserError);
        assert_eq!(result.error, Some("No conversion from text to json".into()));
        options
            .converters
            .push(("text json".into(), Converter::new(|_| Err("bad".into()))));
        assert_eq!(
            convert_response(&options, "x".into(), false).unwrap().error,
            Some("bad".into())
        );
        options.throws = true;
        assert_eq!(
            convert_response(&options, "x".into(), true),
            Err(ConversionFailure {
                error: "bad".into(),
                response_fields: vec![("responseText".into(), "x".into())]
            })
        );
        options.data_filter = Some(Rc::new(|_, _| Err("filter".into())));
        options.throws = false;
        assert_eq!(
            convert_response(&options, "x".into(), true),
            Err(ConversionFailure {
                error: "filter".into(),
                response_fields: vec![("responseText".into(), "x".into())]
            })
        );
    }
    #[test]
    fn content_negotiation_inserts_actual_response_stage() {
        let mut options = ConversionOptions {
            data_types: vec!["*".into()],
            converters: vec![("text json".into(), Converter::Identity)],
            ..Default::default()
        };
        let response = select_response(
            &mut options,
            &[("text".into(), "{}".into())],
            Some("application/json"),
            None,
            &[("json".into(), true)],
        );
        assert_eq!(response, Some("{}".into()));
        assert_eq!(
            options.data_types,
            vec!["text".to_string(), "json".to_string()]
        );
    }
}
