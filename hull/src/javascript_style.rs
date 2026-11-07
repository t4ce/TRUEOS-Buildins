//! Inline CSS through Lightning CSS; each authored field commits only once.
use alloc::{string::{String, ToString}, vec::Vec, borrow::ToOwned, format, vec};
use super::{DomResult, Gateway, MAX_STRING_BYTES, failure};
use lightningcss::{
    declaration::DeclarationBlock,
    properties::{
        Property, PropertyId,
        custom::{Token, TokenList, TokenOrValue},
    },
    stylesheet::{ParserOptions, PrinterOptions},
};
use serde_json::{Value, json};
use static_self::IntoOwned;
use hashbrown::HashSet;
pub(super) type Locks = alloc::rc::Rc<core::cell::RefCell<HashSet<(usize, String)>>>;
const MAX_FIELDS: usize = 4096;
const MAX_DECLARATIONS: usize = 256;
fn valid(property: &Property) -> bool {
    let Property::Unparsed(p) = property else {
        return true;
    };
    if p.value.0.len() == 1 {
        if let TokenOrValue::Token(Token::Ident(name)) = &p.value.0[0] {
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "initial" | "inherit" | "unset" | "revert" | "revert-layer"
            ) {
                return true;
            }
        }
    }
    let mut lists: Vec<&TokenList> = vec![&p.value];
    let mut count = 0;
    while let Some(list) = lists.pop() {
        for token in &list.0 {
            count += 1;
            if count > 4096 {
                return false;
            }
            match token {
                TokenOrValue::Var(_) | TokenOrValue::Env(_) => return true,
                TokenOrValue::Function(f) => lists.push(&f.arguments),
                _ => {}
            }
        }
    }
    false
}
fn bounded(block: &DeclarationBlock) -> DomResult<()> {
    if block.len() > MAX_DECLARATIONS {
        Err(failure(
            "QuotaExceededError",
            "Inline CSS declaration budget exceeded",
        ))
    } else {
        Ok(())
    }
}
fn clean(block: &mut DeclarationBlock) {
    let keep = |p: &Property| valid(p) && supported(p.property_id().name());
    block.declarations.retain(keep);
    block.important_declarations.retain(keep);
}
fn options<'a>() -> ParserOptions<'a> {
    ParserOptions {
        error_recovery: true,
        ..ParserOptions::default()
    }
}
fn supported(name: &str) -> bool {
    !matches!(PropertyId::from(name), PropertyId::Custom(_))
        || name.starts_with("--") && name.len() > 2
}
fn fields(name: &str) -> Vec<String> {
    PropertyId::from(name).longhands().map_or_else(
        || vec![name.to_owned()],
        |ids| ids.iter().map(|id| id.name().to_owned()).collect(),
    )
}
fn encode(block: &DeclarationBlock) -> DomResult<String> {
    block
        .iter()
        .map(|(p, important)| {
            p.to_css_string(important, PrinterOptions::default())
                .map_err(|e| failure("SyntaxError", e.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|v| v.join("; "))
}
pub(super) fn attribute_locks(
    gateway: &Gateway,
    id: usize,
    text: &str,
) -> DomResult<Vec<(usize, String)>> {
    if text.len() > MAX_STRING_BYTES {
        return Err(failure(
            "QuotaExceededError",
            "Inline CSS string budget exceeded",
        ));
    }
    let block =
        DeclarationBlock::parse_string(text, options()).unwrap_or_else(|_| DeclarationBlock::new());
    bounded(&block)?;
    let mut keys = HashSet::new();
    for (property, _) in block.iter() {
        let name = property.property_id();
        if valid(property) && supported(name.name()) {
            keys.extend(fields(name.name()).into_iter().map(|name| (id, name)));
            if name.name() == "all" {
                keys.insert((id, "*".into()));
            }
        }
    }
    if gateway.style_locks.borrow().len() + keys.len() > MAX_FIELDS {
        return Err(failure(
            "QuotaExceededError",
            "Inline CSS field budget exceeded",
        ));
    }
    Ok(keys.into_iter().collect())
}
pub(super) fn call(
    gateway: &mut Gateway,
    id: usize,
    action: &str,
    raw: &str,
    value: &str,
    priority: &str,
) -> DomResult<Value> {
    if raw.len() > MAX_STRING_BYTES || value.len() > MAX_STRING_BYTES {
        return Err(failure(
            "QuotaExceededError",
            "Inline CSS string budget exceeded",
        ));
    }
    let text = {
        let doc = gateway.document.borrow();
        let (_, _, attrs) = doc
            .element(id)
            .ok_or_else(|| failure("TypeError", "Expected an Element"))?;
        attrs
            .iter()
            .find(|a| a.namespace.is_none() && a.name == "style")
            .map(|a| a.value.clone())
            .unwrap_or_default()
    };
    if text.len() > MAX_STRING_BYTES {
        return Err(failure(
            "QuotaExceededError",
            "Inline CSS string budget exceeded",
        ));
    }
    let mut block = DeclarationBlock::parse_string(&text, options())
        .unwrap_or_else(|_| DeclarationBlock::new());
    bounded(&block)?;
    clean(&mut block);
    let name = if raw.starts_with("--") {
        raw.to_owned()
    } else {
        raw.to_ascii_lowercase()
    };
    let property = PropertyId::from(name.as_str());
    let previous = block
        .get(&property)
        .map(|(p, _)| {
            p.value_to_css_string(PrinterOptions::default())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    match action {
        "supports" => return Ok(json!(supported(&name))),
        "get" => return Ok(json!(previous)),
        "priority" => {
            return Ok(json!(
                if block.get(&property).is_some_and(|(_, important)| important) {
                    "important"
                } else {
                    ""
                }
            ));
        }
        "text" => return Ok(json!(encode(&block)?)),
        "names" => {
            return Ok(json!(
                block
                    .iter()
                    .map(|(p, _)| p.property_id().name().to_owned())
                    .collect::<Vec<_>>()
            ));
        }
        _ => {}
    }
    let mut changes = Vec::new();
    let mut incoming = None;
    if action == "cssText" {
        let mut parsed = DeclarationBlock::parse_string(value, options())
            .unwrap_or_else(|_| DeclarationBlock::new());
        bounded(&parsed)?;
        clean(&mut parsed);
        changes.extend(block.iter().map(|(p, _)| p.property_id().name().to_owned()));
        changes.extend(
            parsed
                .iter()
                .map(|(p, _)| p.property_id().name().to_owned()),
        );
        incoming = Some(parsed.into_owned());
    } else if action == "set" || action == "remove" {
        if !supported(&name) {
            return Ok(json!(""));
        }
        if action == "set" && !value.is_empty() {
            if !priority.is_empty() && !priority.eq_ignore_ascii_case("important") {
                return Ok(Value::Null);
            }
            let code = format!(
                "{name}: {value}{}",
                if priority.is_empty() {
                    ""
                } else {
                    " !important"
                }
            );
            let parsed = DeclarationBlock::parse_string(&code, options())
                .unwrap_or_else(|_| DeclarationBlock::new());
            if parsed.len() != 1
                || !parsed.iter().all(|(p, important)| {
                    valid(p) && p.property_id() == property && important == !priority.is_empty()
                })
            {
                return Ok(Value::Null);
            }
            incoming = Some(parsed.into_owned());
        }
        changes.push(name.clone());
    } else {
        return Err(failure(
            "NotSupportedError",
            "Unsupported inline CSS operation",
        ));
    }
    let mut locked = Vec::new();
    let mut seen = HashSet::new();
    for name in changes {
        if !seen.insert(name.clone()) || !supported(&name) {
            continue;
        }
        let keys = fields(&name);
        let locks = gateway.style_locks.borrow();
        if locks.contains(&(id, "*".into()))
            || keys.iter().any(|key| locks.contains(&(id, key.clone())))
            || name == "all" && locks.iter().any(|(node, _)| *node == id)
        {
            continue;
        }
        drop(locks);
        let pid = PropertyId::from(name.as_str());
        block.remove(&pid);
        if let Some((p, important)) = incoming.as_ref().and_then(|b| b.get(&pid)) {
            block.set(p.into_owned(), important);
        }
        locked.extend(keys.into_iter().map(|key| (id, key)));
        if name == "all" {
            locked.push((id, "*".into()));
        }
    }
    if locked.is_empty() {
        return Ok(if action == "remove" {
            json!(previous)
        } else {
            Value::Null
        });
    }
    if gateway.style_locks.borrow().len() + locked.len() > MAX_FIELDS {
        return Err(failure(
            "QuotaExceededError",
            "Inline CSS field budget exceeded",
        ));
    }
    bounded(&block)?;
    let serialized = encode(&block)?;
    // Write through the ordinary attribute path, then latch the successful fields.
    // The private raw operation avoids recursion through cssText admission.
    gateway.call(&json!([
        "attribute",
        id,
        "styleCommit",
        "style",
        serialized
    ]))?;
    gateway.style_locks.borrow_mut().extend(locked);
    Ok(if action == "remove" {
        json!(previous)
    } else {
        Value::Null
    })
}
