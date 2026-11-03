//! One navigation-local box for cookies and Web Storage. No disk reads or network integration.
use std::{cell::RefCell, rc::Rc};

pub(crate) type SharedCookies = Rc<RefCell<CookieJar>>;
pub(crate) const MAX_COOKIE_BYTES: usize = 1024 * 1024;
#[derive(Default)]
pub(crate) struct CookieJar {
    pairs: Vec<(String, String)>,
    version: u64,
    charged: usize,
    full: bool,
}
impl CookieJar {
    /// Changes only when the externally visible pairs change, not on reads,
    /// rejected writes, quota accounting, or setters retaining the same value.
    pub fn version(&self) -> u64 {
        self.version
    }

    fn changed(&mut self) {
        self.version += 1;
    }

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
        self.storage_namespace("localStorage:", false, action, key, value)
    }
    pub fn session_storage(
        &mut self,
        action: &str,
        key: &str,
        value: &str,
    ) -> Option<serde_json::Value> {
        self.storage_namespace("sessionStorage:", true, action, key, value)
    }
    fn storage_namespace(
        &mut self,
        prefix: &str,
        read_only: bool,
        action: &str,
        key: &str,
        value: &str,
    ) -> Option<serde_json::Value> {
        use serde_json::json;
        if read_only && matches!(action, "set" | "remove" | "clear") {
            return Some(if action == "set" {
                json!(true)
            } else {
                serde_json::Value::Null
            });
        }
        let internal = format!("{prefix}{key}");
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
                    .filter_map(|(key, _)| key.strip_prefix(prefix))
                    .collect::<Vec<_>>()
            ),
            "length" => json!(
                self.pairs
                    .iter()
                    .filter(|(key, _)| key.starts_with(prefix))
                    .count()
            ),
            "key" => json!(key.parse::<usize>().ok().and_then(|i| {
                self.pairs
                    .iter()
                    .filter_map(|(key, _)| key.strip_prefix(prefix))
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
                    if old != value {
                        *old = value.to_owned();
                        self.changed();
                    }
                } else {
                    self.pairs.push((internal, value.to_owned()));
                    self.changed();
                }
                json!(true)
            }
            "remove" => {
                let previous = self.pairs.len();
                self.pairs.retain(|(name, _)| name != &internal);
                if self.pairs.len() != previous {
                    self.changed();
                }
                serde_json::Value::Null
            }
            "clear" => {
                let previous = self.pairs.len();
                self.pairs.retain(|(name, _)| !name.starts_with(prefix));
                if self.pairs.len() != previous {
                    self.changed();
                }
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
                self.changed();
            } else if self.pairs[index].1 != value {
                self.pairs[index].1 = value.to_owned();
                self.changed();
            }
        } else if !delete {
            self.pairs.push((name.to_owned(), value.to_owned()));
            self.changed();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_tracks_committed_pair_changes_only() {
        let mut jar = CookieJar::default();
        assert_eq!(jar.version(), 0);
        for invalid in [
            "missing-equals",
            "=empty",
            "bad name=value",
            "a=bad\nvalue",
            "absent=; Max-Age=0",
        ] {
            jar.set(invalid);
        }
        assert_eq!(jar.version(), 0);
        jar.set("a=one");
        assert_eq!(jar.version(), 1);
        jar.set("a=one; Path=/");
        jar.storage("get", "a", "");
        jar.storage("remove", "absent", "");
        jar.storage("clear", "", "");
        jar.session_storage("set", "ignored", "value");
        assert_eq!(jar.version(), 1);
        jar.set("a=two");
        jar.set("a=; Max-Age=0");
        assert_eq!(jar.version(), 3);
        jar.storage("set", "key", "one");
        assert_eq!(jar.version(), 4);
        jar.storage("set", "key", "one");
        assert_eq!(jar.version(), 4);
        jar.storage("set", "key", "two");
        jar.storage("remove", "key", "");
        jar.storage("set", "key", "three");
        jar.storage("clear", "", "");
        assert_eq!(jar.version(), 8);
        jar.set("keep=value");
        let version = jar.version();
        jar.ignore_further_writes();
        jar.set("keep=changed");
        jar.storage("set", "blocked", "value");
        assert_eq!(jar.version(), version);
        assert_eq!(jar.get(), "keep=value");
    }

    #[test]
    fn session_view_reads_its_prefix_without_mutating_or_spending_budget() {
        let mut jar = CookieJar::default();
        jar.storage("set", "same", "local");
        jar.pairs
            .push(("sessionStorage:same".into(), "session".into()));
        let charged = jar.charged;
        assert_eq!(
            jar.session_storage("get", "same", ""),
            Some(serde_json::json!("session"))
        );
        assert_eq!(
            jar.session_storage("keys", "", ""),
            Some(serde_json::json!(["same"]))
        );
        jar.session_storage("set", "same", "changed");
        jar.session_storage("remove", "same", "");
        jar.session_storage("clear", "", "");
        assert_eq!(
            jar.session_storage("get", "same", ""),
            Some(serde_json::json!("session"))
        );
        assert_eq!(
            jar.storage("get", "same", ""),
            Some(serde_json::json!("local"))
        );
        assert_eq!(jar.charged, charged);
    }
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
