//! One navigation-local box for cookies and Web Storage. No disk reads or network integration.
use std::{cell::RefCell, rc::Rc};

pub(crate) type SharedCookies = Rc<RefCell<CookieJar>>;
pub(crate) const MAX_COOKIE_BYTES: usize = 1024 * 1024;
#[derive(Default)]
pub(crate) struct CookieJar {
    pairs: Vec<(String, String)>,
    charged: usize,
    full: bool,
}
impl CookieJar {
    fn charge(&mut self, bytes: usize) -> bool {
        if self.full || bytes > MAX_COOKIE_BYTES.saturating_sub(self.charged) {
            self.full = true;
            return false;
        }
        self.charged += bytes;
        true
    }
    pub fn snapshot(&self) -> String {
        serde_json::to_string(&self.pairs).expect("string pairs serialize")
    }
    pub fn storage(&mut self, action: &str, key: &str, value: &str) -> Option<serde_json::Value> {
        use serde_json::json;
        let internal = format!("localStorage:{key}");
        Some(match action {
            "get" => json!(
                self.pairs
                    .iter()
                    .find(|(name, _)| name == &internal)
                    .map(|(_, value)| value)
            ),
            "keys" => json!(
                self.pairs
                    .iter()
                    .filter_map(|(key, _)| key.strip_prefix("localStorage:"))
                    .collect::<Vec<_>>()
            ),
            "length" => json!(
                self.pairs
                    .iter()
                    .filter(|(key, _)| key.starts_with("localStorage:"))
                    .count()
            ),
            "key" => json!(key.parse::<usize>().ok().and_then(|i| {
                self.pairs
                    .iter()
                    .filter_map(|(key, _)| key.strip_prefix("localStorage:"))
                    .nth(i)
            })),
            "set" => {
                let bytes = serde_json::to_string(&(&internal, value))
                    .ok()?
                    .len()
                    .saturating_add(32);
                if !self.charge(bytes) {
                    return Some(json!(false));
                }
                if let Some((_, old)) = self.pairs.iter_mut().find(|(name, _)| name == &internal) {
                    *old = value.to_owned();
                } else {
                    self.pairs.push((internal, value.to_owned()));
                }
                json!(true)
            }
            "remove" => {
                self.pairs.retain(|(name, _)| name != &internal);
                serde_json::Value::Null
            }
            "clear" => {
                self.pairs
                    .retain(|(name, _)| !name.starts_with("localStorage:"));
                serde_json::Value::Null
            }
            _ => return None,
        })
    }
    pub fn ignore_further_writes(&mut self) {
        self.full = true;
    }
    pub fn get(&self) -> String {
        self.pairs
            .iter()
            .filter_map(|(n, v)| n.strip_prefix("cookie:").map(|n| format!("{n}={v}")))
            .collect::<Vec<_>>()
            .join("; ")
    }
    pub fn set(&mut self, text: &str) {
        // Cumulative JSON-encoded UTF-8 setter bytes plus 32 bytes per attempt: replacements,
        // deletion and invalid input also consume budget. Once exceeded, latch.
        let charge = serde_json::to_string(text)
            .map_or(usize::MAX, |encoded| encoded.len())
            .saturating_add(32);
        if !self.charge(charge) {
            return;
        }
        let mut parts = text.split(';');
        let Some((name, value)) = parts.next().unwrap_or("").split_once('=') else {
            return;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty()
            || name
                .bytes()
                .any(|b| b <= 32 || b >= 127 || b"()<>@,:\\\"/[]?={}".contains(&b))
            || value.chars().any(|c| c.is_control())
        {
            return;
        }
        let mut delete = false;
        for attribute in parts {
            if let Some((key, value)) = attribute.trim().split_once('=') {
                if key.trim().eq_ignore_ascii_case("max-age") {
                    if let Ok(age) = value.trim().parse::<i64>() {
                        delete = age <= 0;
                    }
                }
            }
        }
        let name = format!("cookie:{name}");
        if let Some(index) = self.pairs.iter().position(|(n, _)| n == &name) {
            if delete {
                self.pairs.remove(index);
            } else {
                self.pairs[index].1 = value.to_owned();
            }
        } else if !delete {
            self.pairs.push((name.to_owned(), value.to_owned()));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cookie_pairs_and_deletion() {
        let mut jar = CookieJar::default();
        jar.set("a=one; Path=/; Secure; SameSite=Lax");
        jar.set("b=two=three");
        jar.set("a=replaced");
        assert_eq!(jar.get(), "a=replaced; b=two=three");
        jar.set("a=; Max-Age=0");
        jar.set("invalid");
        jar.set("=empty");
        assert_eq!(jar.get(), "b=two=three");
        assert_eq!(CookieJar::default().get(), "");
    }
    #[test]
    fn cumulative_utf8_budget_latches() {
        let mut jar = CookieJar::default();
        jar.set("a=é");
        assert_eq!(jar.charged, 38);
        jar.set(&format!("b={}", "x".repeat(MAX_COOKIE_BYTES - 38 - 36)));
        assert_eq!(jar.charged, MAX_COOKIE_BYTES);
        jar.set("a=deleted; Max-Age=0");
        jar.set("a=changed");
        assert!(jar.get().starts_with("a=é; b="));
        assert!(jar.full);
        assert!(
            serde_json::to_vec(&serde_json::json!({"ok":true,"value":jar.get()}))
                .unwrap()
                .len()
                <= MAX_COOKIE_BYTES
        );
    }
}
