//! Cascaded, inherited custom properties. Lightning CSS owns token parsing and substitution.
use alloc::{string::{String, ToString}, vec::Vec, borrow::ToOwned};
use lightningcss::properties::{
    Property, PropertyId,
    custom::{CustomPropertyName, TokenList, TokenOrValue, Variable},
};
use lightningcss::visitor::{Visit, VisitTypes, Visitor};
use hashbrown::{HashMap, HashSet};
use alloc::sync::Arc;

const MAX_TOKENS: usize = 8192;
const MAX_DEPTH: usize = 32;

#[derive(Clone, Default)]
pub(super) struct Variables(Arc<HashMap<String, TokenList<'static>>>);

impl Variables {
    pub fn cascade<'a>(&mut self, properties: impl Iterator<Item = &'a Property<'static>>) {
        let mut raw = None;
        for property in properties {
            let Property::Custom(custom) = property else {
                continue;
            };
            if !matches!(custom.name, CustomPropertyName::Custom(_)) {
                continue;
            }
            let name = custom.name.as_ref();
            let map = raw.get_or_insert_with(|| (*self.0).clone());
            let words: Vec<_> = custom
                .value
                .0
                .iter()
                .filter(|token| !token.is_whitespace())
                .collect();
            match words.as_slice() {
                [TokenOrValue::Token(lightningcss::properties::custom::Token::Ident(word))]
                    if word.eq_ignore_ascii_case("initial") =>
                {
                    map.remove(name);
                }
                [TokenOrValue::Token(lightningcss::properties::custom::Token::Ident(word))]
                    if word.eq_ignore_ascii_case("inherit")
                        || word.eq_ignore_ascii_case("unset") =>
                {
                    if let Some(value) = self.0.get(name) {
                        map.insert(name.to_owned(), value.clone());
                    } else {
                        map.remove(name);
                    }
                }
                _ => {
                    map.insert(name.to_owned(), custom.value.clone());
                }
            }
        }
        let Some(raw) = raw else {
            return;
        };
        let mut resolver = Resolver {
            raw: &raw,
            done: HashMap::new(),
            invalid: HashSet::new(),
            active: Vec::new(),
        };
        for name in raw.keys() {
            resolver.resolve(name);
        }
        self.0 = Arc::new(
            resolver
                .done
                .into_iter()
                .map(|(name, (value, _))| (name, value))
                .collect(),
        );
    }

    pub fn property(
        &self,
        property: &lightningcss::properties::custom::UnparsedProperty<'static>,
    ) -> Option<Property<'static>> {
        let mut value = property.value.clone();
        let info = inspect(&mut value)?;
        let mut budget = info.count;
        let mut vars = lightningcss::collections::HashMap::new();
        for name in info.names {
            if let Some(value) = self.0.get(&name) {
                budget = budget.checked_add(inspect(&mut value.clone())?.count)?;
                if budget > MAX_TOKENS {
                    return None;
                }
                vars.insert(name, value.clone());
            }
        }
        let vars = vars
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        let resolved = property
            .clone()
            .substitute_variables(&vars)
            .ok()
            .filter(|property| !matches!(property, Property::Unparsed(_)));
        resolved.or_else(|| match property.property_id {
            // An invalid winning background computes to its initial value;
            // it must not reveal an earlier declaration from the cascade.
            PropertyId::Background | PropertyId::BackgroundColor => Property::parse_string(
                property.property_id.clone(),
                "transparent",
                Default::default(),
            )
            .ok(),
            _ => None,
        })
    }
}

struct Resolver<'a> {
    raw: &'a HashMap<String, TokenList<'static>>,
    done: HashMap<String, (TokenList<'static>, usize)>,
    invalid: HashSet<String>,
    active: Vec<String>,
}

impl Resolver<'_> {
    fn resolve(&mut self, name: &str) {
        if self.done.contains_key(name) || self.invalid.contains(name) {
            return;
        }
        if let Some(start) = self.active.iter().position(|active| active == name) {
            self.invalid.extend(self.active[start..].iter().cloned());
            return;
        }
        if self.active.len() >= MAX_DEPTH {
            self.invalid.insert(name.to_owned());
            return;
        }
        let Some(mut value) = self.raw.get(name).cloned() else {
            return;
        };
        let Some(info) = inspect(&mut value) else {
            self.invalid.insert(name.to_owned());
            return;
        };
        self.active.push(name.to_owned());
        for dependency in &info.names {
            self.resolve(dependency);
        }
        self.active.pop();
        if self.invalid.contains(name) {
            return;
        }
        let mut budget = info.count;
        let mut vars = lightningcss::collections::HashMap::new();
        for dependency in &info.names {
            if let Some((tokens, count)) = self.done.get(dependency) {
                budget = budget.saturating_add(*count);
                if budget > MAX_TOKENS {
                    self.invalid.insert(name.to_owned());
                    return;
                }
                vars.insert(dependency.as_str(), tokens.clone());
            }
        }
        value.substitute_variables(&vars);
        if let Some(info) = inspect(&mut value) {
            if info.names.is_empty() {
                self.done.insert(name.to_owned(), (value, info.count));
                return;
            }
        }
        self.invalid.insert(name.to_owned());
    }
}

#[derive(Default)]
struct References {
    names: Vec<String>,
    count: usize,
    depth: usize,
}
impl<'i> Visitor<'i> for References {
    type Error = ();
    fn visit_types(&self) -> VisitTypes {
        VisitTypes::TOKENS | VisitTypes::VARIABLES
    }
    fn visit_variable(&mut self, var: &mut Variable<'i>) -> Result<(), ()> {
        self.names.push(var.name.ident.0.to_string());
        var.visit_children(self)
    }
    fn visit_token_list(&mut self, tokens: &mut TokenList<'i>) -> Result<(), ()> {
        self.count = self.count.saturating_add(tokens.0.len());
        self.depth += 1;
        if self.count > MAX_TOKENS || self.depth > MAX_DEPTH {
            return Err(());
        }
        tokens.visit_children(self)?;
        self.depth -= 1;
        Ok(())
    }
}
fn inspect(tokens: &mut TokenList<'_>) -> Option<References> {
    let mut refs = References::default();
    tokens.visit(&mut refs).ok()?;
    Some(refs)
}
