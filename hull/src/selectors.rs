//! Shared static CSS matching and bounded selectors for the script-visible DOM.
use lightningcss::selector::{Component, Selector, SelectorList};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::ParseWithOptions;
use rp5::{Document, NodeId as HtmlId, NodeKind};
use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Default)]
struct SiblingPosition {
    previous: Option<HtmlId>,
    index: usize,
    count: usize,
    type_index: usize,
    type_count: usize,
}

/// Immutable DOM facts shared by every selector match in this layout pass.
pub(crate) struct MatchDom<'a> {
    query: bool,
    scope: Option<HtmlId>,
    remaining: Cell<usize>,
    deadline: Option<Instant>,
    exhausted: Cell<bool>,
    attributes: Vec<HashMap<&'a str, &'a str>>,
    classes: Vec<std::collections::HashSet<&'a str>>,
    siblings: Vec<SiblingPosition>,
}

impl<'a> MatchDom<'a> {
    pub(crate) fn new(document: &'a Document) -> Self {
        let mut attributes = Vec::with_capacity(document.nodes.len());
        let mut classes = Vec::with_capacity(document.nodes.len());
        for id in 0..document.nodes.len() {
            let mut node_attributes = HashMap::new();
            let mut node_classes = std::collections::HashSet::new();
            if let Some((_, _, attrs)) = document.element(id) {
                for attribute in attrs
                    .iter()
                    .filter(|attribute| attribute.namespace.is_none())
                {
                    // Preserve the matcher's first-attribute behavior.
                    node_attributes
                        .entry(attribute.name.as_str())
                        .or_insert(attribute.value.as_str());
                }
                if let Some(value) = node_attributes.get("class") {
                    node_classes.extend(value.split_ascii_whitespace());
                }
            }
            attributes.push(node_attributes);
            classes.push(node_classes);
        }
        let mut siblings = vec![SiblingPosition::default(); document.nodes.len()];
        for parent in &document.nodes {
            let mut previous = None;
            let mut count = 0;
            let mut types = HashMap::new();
            for &id in &parent.children {
                if let Some((name, namespace, _)) = document.element(id) {
                    count += 1;
                    let type_count = types.entry((namespace as u8, name)).or_insert(0);
                    *type_count += 1;
                    siblings[id] = SiblingPosition {
                        previous,
                        index: count,
                        type_index: *type_count,
                        ..SiblingPosition::default()
                    };
                    previous = Some(id);
                }
            }
            for &id in &parent.children {
                if let Some((name, namespace, _)) = document.element(id) {
                    siblings[id].count = count;
                    siblings[id].type_count = types[&(namespace as u8, name)];
                }
            }
        }
        Self {
            query: false,
            scope: None,
            remaining: Cell::new(usize::MAX),
            deadline: None,
            exhausted: Cell::new(false),
            attributes,
            classes,
            siblings,
        }
    }
}

fn matches_selector(
    document: &Document,
    dom: &MatchDom<'_>,
    id: HtmlId,
    selector: &Selector<'_>,
) -> bool {
    let components: Vec<_> = selector.iter_raw_match_order().cloned().collect();
    matches_components(document, dom, id, &components, &mut Vec::new())
}

pub(crate) fn matches_components(
    document: &Document,
    dom: &MatchDom<'_>,
    id: HtmlId,
    components: &[lightningcss::selector::Component<'_>],
    pending: &mut Vec<(HtmlId, usize)>,
) -> bool {
    use lightningcss::selector::PseudoClass;
    use parcel_selectors::parser::Combinator;
    pending.clear();
    pending.push((id, 0));
    while let Some((id, start)) = pending.pop() {
        if !dom.step() {
            return false;
        }
        let Some((name, namespace, _)) = document.element(id) else {
            continue;
        };
        let attr = |name: &str| dom.attributes[id].get(name).copied();
        let mut matched = true;
        for (offset, component) in components.iter().enumerate().skip(start) {
            if !dom.step() {
                return false;
            }
            if let Component::Combinator(combinator) = component {
                let next = offset + 1;
                match combinator {
                    Combinator::Child => {
                        if let Some(parent) = document.nodes[id]
                            .parent
                            .filter(|&parent| document.element(parent).is_some())
                        {
                            pending.push((parent, next));
                        }
                    }
                    Combinator::Descendant => {
                        let mut parent = document.nodes[id].parent;
                        while let Some(id) = parent {
                            if !dom.step() {
                                return false;
                            }
                            if document.element(id).is_some() {
                                pending.push((id, next));
                            }
                            parent = document.nodes[id].parent;
                        }
                    }
                    Combinator::NextSibling | Combinator::LaterSibling => {
                        let mut previous = dom.siblings[id].previous;
                        while let Some(sibling) = previous {
                            if !dom.step() {
                                return false;
                            }
                            pending.push((sibling, next));
                            if matches!(combinator, Combinator::NextSibling) {
                                break;
                            }
                            previous = dom.siblings[sibling].previous;
                        }
                    }
                    _ => {}
                }
                matched = false;
                break;
            }
            let ok=match component {
                Component::ExplicitUniversalType|Component::ExplicitAnyNamespace=>true,
                Component::LocalName(local)=>if dom.query && namespace==rp5::Namespace::Html {name.eq_ignore_ascii_case(local.name.as_ref())} else {name==local.name.as_ref()},
                Component::ID(value)=>attr("id").is_some_and(|actual|dom.ident_matches(document,actual,value.as_ref())),
                Component::Class(value)=>if dom.query && document.mode==rp5::DocumentMode::Quirks {dom.classes[id].iter().any(|actual|actual.eq_ignore_ascii_case(value.as_ref()))} else {dom.classes[id].contains(value.as_ref())},
                Component::AttributeInNoNamespaceExists{local_name,local_name_lower}=>attr(if dom.query && namespace==rp5::Namespace::Html {local_name_lower.as_ref()} else {local_name.as_ref()}).is_some(),
                Component::AttributeInNoNamespace{local_name,operator,value,case_sensitivity,never_matches,..}=>!never_matches && attr(local_name.as_ref()).is_some_and(|actual|operator.eval_str(actual,value.as_ref(),case_sensitivity.to_unconditional(namespace==rp5::Namespace::Html))),
                Component::Where(selectors)|Component::Is(selectors)=>selectors.iter().any(|selector|matches_selector(document,dom,id,selector)),
                Component::Negation(selectors)=>!selectors.iter().any(|selector|matches_selector(document,dom,id,selector)),
                Component::Root=>document.nodes[id].parent==Some(document.root),
                Component::Scope=>dom.scope==Some(id),
                Component::ExplicitNoNamespace if dom.query=>false,
                Component::AttributeOther(other) if dom.query=>match_attribute(document,id,other),
                Component::Empty=>document.nodes[id].children.iter().all(|&child|matches!(&document.nodes[child].kind,NodeKind::Comment(_)) || matches!(&document.nodes[child].kind,NodeKind::Text(text) if text.is_empty())),
                Component::Nth(nth)=>{
                    let siblings = dom.siblings[id];
                    let (index, count) = if nth.ty.is_of_type() { (siblings.type_index, siblings.type_count) } else { (siblings.index, siblings.count) };
                    if nth.ty.is_only() { count == 1 } else if index > 0 {
                        let position = if nth.ty.is_from_end() { count-index+1 } else { index } as i64;
                        let delta = position - i64::from(nth.b);
                        let a = i64::from(nth.a);
                        if a == 0 { delta == 0 } else { delta % a == 0 && delta / a >= 0 }
                    } else { false }
                }
                Component::NonTSPseudoClass(pseudo)=>match pseudo {
                    PseudoClass::Link|PseudoClass::AnyLink(_)=>matches!(name,"a"|"area"|"link") && attr("href").is_some(),
                    PseudoClass::Disabled=>attr("disabled").is_some(),
                    PseudoClass::Enabled=>matches!(name,"button"|"input"|"select"|"textarea"|"option"|"optgroup") && attr("disabled").is_none(),
                    PseudoClass::Checked=>attr("checked").is_some() || (name=="option" && attr("selected").is_some()),
                    _=>false,
                },
                _=>false,
            };
            if !ok {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

impl MatchDom<'_> {
    fn step(&self) -> bool {
        if !self.query {
            return true;
        }
        let remaining = self.remaining.get();
        if remaining == 0
            || remaining % 256 == 0
                && self
                    .deadline
                    .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.exhausted.set(true);
            return false;
        }
        self.remaining.set(remaining - 1);
        true
    }
    fn ident_matches(&self, document: &Document, actual: &str, expected: &str) -> bool {
        if self.query && document.mode == rp5::DocumentMode::Quirks {
            actual.eq_ignore_ascii_case(expected)
        } else {
            actual == expected
        }
    }
}

fn match_attribute<'i, Impl: parcel_selectors::parser::SelectorImpl<'i>>(
    document: &Document,
    id: HtmlId,
    selector: &parcel_selectors::attr::AttrSelectorWithOptionalNamespace<'i, Impl>,
) -> bool
where
    Impl::LocalName: AsRef<str>,
    Impl::NamespaceUrl: AsRef<str>,
    Impl::AttrValue: AsRef<str>,
{
    use parcel_selectors::attr::{NamespaceConstraint, ParsedAttrSelectorOperation};
    let Some((_, namespace, attributes)) = document.element(id) else {
        return false;
    };
    if selector.never_matches {
        return false;
    }
    let local = if namespace == rp5::Namespace::Html {
        selector.local_name_lower.as_ref()
    } else {
        selector.local_name.as_ref()
    };
    attributes.iter().any(|attribute| {
        let namespace_matches = match selector.namespace() {
            None => attribute.namespace.is_none(),
            Some(NamespaceConstraint::Any) => true,
            Some(NamespaceConstraint::Specific(url)) => {
                attribute.namespace.as_deref().unwrap_or("") == url.as_ref()
            }
        };
        namespace_matches
            && attribute.name == local
            && match &selector.operation {
                ParsedAttrSelectorOperation::Exists => true,
                ParsedAttrSelectorOperation::WithValue {
                    operator,
                    case_sensitivity,
                    expected_value,
                } => operator.eval_str(
                    &attribute.value,
                    expected_value.as_ref(),
                    case_sensitivity.to_unconditional(namespace == rp5::Namespace::Html),
                ),
            }
    })
}

fn validate(selector: &Selector<'_>, depth: usize, count: &mut usize) -> Result<(), String> {
    if depth > 32 {
        return Err("QuotaExceededError: selector nesting exceeds 32 levels".into());
    }
    for component in selector.iter_raw_match_order() {
        *count += 1;
        if *count > 256 {
            return Err("QuotaExceededError: selector exceeds 256 components".into());
        }
        match component {
            Component::ExplicitUniversalType
            | Component::ExplicitAnyNamespace
            | Component::ExplicitNoNamespace
            | Component::LocalName(_)
            | Component::ID(_)
            | Component::Class(_)
            | Component::AttributeInNoNamespaceExists { .. }
            | Component::AttributeInNoNamespace { .. }
            | Component::AttributeOther(_)
            | Component::Root
            | Component::Empty
            | Component::Scope
            | Component::Nth(_)
            | Component::PseudoElement(_) => {}
            Component::Combinator(combinator)
                if matches!(
                    combinator,
                    parcel_selectors::parser::Combinator::Child
                        | parcel_selectors::parser::Combinator::Descendant
                        | parcel_selectors::parser::Combinator::NextSibling
                        | parcel_selectors::parser::Combinator::LaterSibling
                        | parcel_selectors::parser::Combinator::PseudoElement
                ) => {}
            Component::Where(selectors)
            | Component::Is(selectors)
            | Component::Negation(selectors) => {
                for selector in selectors.iter() {
                    validate(selector, depth + 1, count)?;
                }
            }
            _ => return Err("NotSupportedError: this selector feature is not implemented".into()),
        }
    }
    Ok(())
}

// Reject excessive nesting before entering the CSS parser's recursive grammar.
// Quoted attribute values, comments and escapes do not contribute nesting.
fn guard_nesting(source: &str) -> Result<(), String> {
    let bytes = source.as_bytes();
    let mut index = 0;
    let mut quote = None;
    let mut depth = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\\' {
            index += 2;
            continue;
        }
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
        } else if byte == b'\'' || byte == b'"' {
            quote = Some(byte);
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index < bytes.len()
                && !(bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/'))
            {
                index += 1;
            }
            index += 2;
            continue;
        } else if byte == b'(' || byte == b'[' {
            depth += 1;
            if depth > 32 {
                return Err("QuotaExceededError: selector nesting exceeds 32 levels".into());
            }
        } else if byte == b')' || byte == b']' {
            depth = depth.saturating_sub(1);
        }
        index += 1;
    }
    Ok(())
}

/// Candidates are receiver descendants; ancestor conditions may inspect the
/// rest of the tree. Results are deduplicated in document order.
pub(crate) fn query_selector_ids(
    document: &Document,
    scope: HtmlId,
    source: &str,
) -> Result<Vec<HtmlId>, String> {
    if scope >= document.nodes.len() {
        return Err("NotFoundError: query receiver is not in this document".into());
    }
    if source.len() > 16 * 1024 || document.nodes.len() > 100_000 {
        return Err("QuotaExceededError: selector or document exceeds query bounds".into());
    }
    guard_nesting(source)?;
    let until = Instant::now() + Duration::from_millis(20);
    let mut input = cssparser::ParserInput::new(source);
    let mut parser = cssparser::Parser::new(&mut input);
    let selectors = SelectorList::parse_with_options(&mut parser, &ParserOptions::default())
        .map_err(|_| "SyntaxError: invalid CSS selector".to_owned())?;
    parser
        .expect_exhausted()
        .map_err(|_| "SyntaxError: invalid CSS selector".to_owned())?;
    let mut components = 0;
    for selector in &selectors.0 {
        validate(selector, 0, &mut components)?;
    }
    let mut dom = MatchDom::new(document);
    if Instant::now() >= until {
        return Err("QuotaExceededError: selector traversal budget exhausted".into());
    }
    dom.query = true;
    dom.scope = if document.element(scope).is_some() {
        Some(scope)
    } else if matches!(document.nodes[scope].kind, NodeKind::Document) {
        document.nodes[scope]
            .children
            .iter()
            .copied()
            .find(|&id| document.element(id).is_some())
    } else {
        None
    };
    dom.remaining.set(1_000_000);
    dom.deadline = Some(until);
    let mut pending: Vec<_> = document.nodes[scope]
        .children
        .iter()
        .rev()
        .copied()
        .collect();
    let mut result = Vec::new();
    while let Some(id) = pending.pop() {
        if !dom.step() {
            break;
        }
        if document.element(id).is_some()
            && selectors
                .0
                .iter()
                .any(|selector| matches_selector(document, &dom, id, selector))
        {
            result.push(id);
        }
        pending.extend(document.nodes[id].children.iter().rev().copied());
    }
    if dom.exhausted.get() || Instant::now() >= until {
        return Err("QuotaExceededError: selector traversal budget exhausted".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ids(document: &Document, source: &str) -> Vec<String> {
        query_selector_ids(document, document.root, source)
            .unwrap()
            .iter()
            .map(|&id| {
                document
                    .element(id)
                    .unwrap()
                    .2
                    .iter()
                    .find(|a| a.name == "id")
                    .map(|a| a.value.clone())
                    .unwrap_or_else(|| document.name(id).into())
            })
            .collect()
    }
    #[test]
    fn selectors_keep_order_deduplicate_and_use_real_css_syntax() {
        let document = rp5::parse(
            "<!doctype html><main><p id='one' class='hit' data-x='a,b'></p><span id='two' class='hit'></span><p id='three'></p></main>",
        );
        assert_eq!(
            ids(&document, "#three, [data-x='a,b'], .hit"),
            ["one", "two", "three"]
        );
        assert_eq!(ids(&document, "main > p + span"), ["two"]);
        assert_eq!(ids(&document, "main p ~ p"), ["three"]);
        assert_eq!(ids(&document, "p:not(#one):is(#three, .absent)"), ["three"]);
        assert_eq!(ids(&document, "main > :nth-child(2)"), ["two"]);
        assert_eq!(ids(&document, "main > p:nth-of-type(2)"), ["three"]);
    }
    #[test]
    fn scope_excludes_receiver_and_can_match_ancestors_outside_it() {
        let document = rp5::parse(
            "<!doctype html><main id='outer'><section id='scope'><p id='child'></p></section></main>",
        );
        let scope = query_selector_ids(&document, document.root, "#scope").unwrap()[0];
        assert!(
            query_selector_ids(&document, scope, ":scope")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            query_selector_ids(&document, scope, ":scope > p").unwrap(),
            query_selector_ids(&document, scope, "main p").unwrap()
        );
        assert_eq!(ids(&document, ":scope"), ["html"]);
        assert_eq!(ids(&document, ":root"), ["html"]);
    }
    #[test]
    fn html_case_and_quirks_matching_preserve_foreign_case() {
        let document = rp5::parse(
            "<!doctype html><div ID='Upper' data-x='YES'></div><svg><linearGradient id='g'/></svg>",
        );
        assert_eq!(ids(&document, "DIV[ID=Upper][DATA-X=yes i]"), ["Upper"]);
        assert_eq!(ids(&document, "#upper"), Vec::<String>::new());
        assert_eq!(ids(&document, "linearGradient"), ["g"]);
        assert_eq!(ids(&document, "lineargradient"), Vec::<String>::new());
        let document = rp5::parse("<p id='Upper' class='Mixed'></p>");
        assert_eq!(ids(&document, "#upper.mixed"), ["Upper"]);
    }
    #[test]
    fn invalid_and_unimplemented_selectors_fail_explicitly() {
        let document = rp5::parse("<!doctype html><p></p>");
        for source in ["", "div[", ",", "div,"] {
            assert!(
                query_selector_ids(&document, document.root, source)
                    .unwrap_err()
                    .starts_with("SyntaxError:"),
                "{source}"
            );
        }
        for source in ["p:has(span)", ":checked", "p:nth-child(1 of p)"] {
            assert!(
                query_selector_ids(&document, document.root, source)
                    .unwrap_err()
                    .starts_with("NotSupportedError:"),
                "{source}"
            );
        }
        assert!(
            query_selector_ids(&document, document.root, "p::before")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn query_guards_nested_parser_work_without_counting_strings_or_comments() {
        let document = rp5::parse(
            "<!doctype html><p data-x='((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((('></p>",
        );
        assert_eq!(
            ids(
                &document,
                "[data-x='(((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((']"
            ),
            ["p"]
        );
        assert_eq!(
            ids(
                &document,
                "/*((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((*/p"
            ),
            ["p"]
        );
        let source = format!("{}p{}", ":not(".repeat(100), ")".repeat(100));
        assert!(
            query_selector_ids(&document, document.root, &source)
                .unwrap_err()
                .starts_with("QuotaExceededError:")
        );
        assert!(ids(&document, "p:nth-child(-2147483648n-2147483648)").is_empty());
    }
}
