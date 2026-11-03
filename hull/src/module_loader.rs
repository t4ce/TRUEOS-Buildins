//! Host-owned, bounded source preparation. No network runs in QuickJS callbacks.
use hull_quickjs::{Engine, ImportRequest};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use trueos::netfs;
use url::Url;

pub(crate) const PRIMARY: &str = "https://esm.sh/";
pub(crate) const SECONDARY: &str = "https://cdn.jsdelivr.net/npm/";
const MAX_FILES: usize = 64;
const MAX_FILE: usize = 1024 * 1024;
const MAX_TOTAL: usize = 8 * 1024 * 1024;
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(5);
const FETCH_WAIT: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(crate) struct Policy {
    page: String,
    aliases: HashMap<String, String>,
    fallbacks: HashMap<String, String>,
    imports: HashMap<String, Option<String>>,
}
pub(crate) fn checked_url(value: &str) -> Result<String, String> {
    if value.len() > 4096 || value.contains('\0') {
        return Err("Module URL exceeds bounds".into());
    }
    let url = Url::parse(value).map_err(|e| e.to_string())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return Err("Module URLs must be HTTPS without credentials".into());
    }
    Ok(url.into())
}
fn package(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 512
        || value
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@/._-~^+".contains(&b))
    {
        return false;
    }
    if value.starts_with('@') {
        value.split('/').count() >= 2
    } else {
        !value.starts_with(['.', '/'])
    }
}
fn mapping_key(base:&Url,name:&str)->Result<String,String>{
    if name.starts_with(['.','/']){base.join(name).map(String::from).map_err(|e|e.to_string())}
    else if let Ok(url)=Url::parse(name){Ok(url.into())}else{Ok(name.into())}
}
impl Policy {
    pub(crate) fn canonical(&self, url: &str) -> String {
        let mut value = url;
        for _ in 0..8 {
            match self.aliases.get(value) {
                Some(next) if next != value => value = next,
                _ => break,
            }
        }
        value.into()
    }
    pub(crate) fn resolve(&mut self, base: &str, name: &str) -> Result<String, String> {
        let base = self.canonical(if !base.starts_with("https://") && !self.page.is_empty() {&self.page}else{base});
        let key=mapping_key(&Url::parse(&base).map_err(|e|e.to_string())?,name)?;
        let mapped = if let Some(mapped) = self.imports.get(&key) {
            Some(mapped.clone().ok_or("Import map blocks this specifier")?)
        } else {
            self.imports
                .iter()
                .filter(|(prefix, _)| prefix.ends_with('/') && key.starts_with(prefix.as_str()))
                .max_by_key(|(prefix, _)| prefix.len())
                .map(|(prefix, value)| -> Result<String,String> {
                    let target=value.as_ref().ok_or_else(||"Import map blocks this prefix".to_owned())?;
                    let resolved=checked_url(&format!("{target}{}",&key[prefix.len()..]))?;
                    if !resolved.starts_with(target){return Err("Import map prefix escapes its target".into());}Ok(resolved)
                })
                .transpose()?
        };
        let name = mapped.as_deref().unwrap_or(name);
        let url = if name.starts_with(['.', '/']) {
            checked_url(
                Url::parse(&base)
                    .and_then(|u| u.join(name))
                    .map_err(|e| e.to_string())?
                    .as_str(),
            )?
        } else if name.contains(':') {
            if let Some(npm) = name.strip_prefix("npm:") {
                return self.npm(npm);
            }
            checked_url(name)?
        } else {
            return self.npm(name);
        };
        // Explicit esm.sh package URLs get the same fallback only when they
        // identify a package entry, never an internal generated build asset.
        let parsed = Url::parse(&url).unwrap();
        if parsed.host_str() == Some("esm.sh") && parsed.query().is_none() {
            let path = parsed.path().trim_start_matches('/');
            let first=path.split('/').next().unwrap_or("");
            if package(path)
                &&!matches!(first,"gh"|"jsr"|"pr")&&!first.starts_with('_')
                &&!(first.starts_with('v')&&first[1..].bytes().all(|b|b.is_ascii_digit()))
                && !path
                    .split('/')
                    .any(|p| p.starts_with("es20") || matches!(p, "node" | "deno" | "denonext"))
            {
                self.fallbacks
                    .entry(url.clone())
                    .or_insert_with(|| format!("{SECONDARY}{path}/+esm"));
            }
        }
        Ok(self.canonical(&url))
    }
    fn npm(&mut self, name: &str) -> Result<String, String> {
        if !package(name) {
            return Err(format!("Unsupported module specifier: {name}"));
        }
        let url = format!("{PRIMARY}{name}?target=es2022&bundle");
        self.fallbacks
            .entry(url.clone())
            .or_insert_with(|| format!("{SECONDARY}{name}/+esm"));
        Ok(self.canonical(&url))
    }
    pub(crate) fn candidates(&mut self, base: &str, name: &str) -> Result<Vec<String>, String> {
        let root = self.resolve(base, name)?;
        let mut result = vec![root.clone()];
        if let Some(fallback) = self.fallbacks.get(&root) {
            result.push(fallback.clone());
        }
        Ok(result)
    }
    fn switch(&mut self, url: &str) -> Option<String> {
        let fallback = self.fallbacks.get(url)?.clone();
        if self.aliases.contains_key(url) {
            return None;
        }
        self.aliases.insert(url.into(), fallback.clone());
        Some(fallback)
    }
    pub(crate) fn document_maps(&mut self, doc: &rp5::Document, page: &str) -> Result<(), String> {
        let mut base = Url::parse(page).map_err(|e| e.to_string())?;
        let mut stack = vec![doc.root];
        let mut order = Vec::new();
        while let Some(id) = stack.pop() {
            order.push(id);
            stack.extend(doc.nodes[id].children.iter().rev().copied());
        }
        for id in &order {
            if let Some(("base", rp5::Namespace::Html, attrs)) = doc.element(*id) {
                if let Some(href) = attrs.iter().find(|a| a.name == "href") {
                    base = base.join(&href.value).map_err(|e| e.to_string())?;
                    break;
                }
            }
        }
        for id in order {
            if let Some(("script", rp5::Namespace::Html, attrs)) = doc.element(id) {
                if attrs
                    .iter()
                    .any(|a| a.name == "type" && a.value == "importmap")
                {
                    let code = doc.text_content(id);
                    if code.len() > 65536 {
                        return Err("Import map exceeds 64 KiB".into());
                    }
                    let map: serde_json::Value =
                        serde_json::from_str(&code).map_err(|e| format!("Import map: {e}"))?;
                    if map
                        .get("scopes")
                        .is_some_and(|v| v.as_object().is_none_or(|m| !m.is_empty()))
                    {
                        return Err("Scoped import maps are not admitted".into());
                    }
                    if let Some(imports) = map.get("imports") {
                        let imports = imports
                            .as_object()
                            .ok_or("Import map imports must be an object")?;
                        if imports.len() > 128 {
                            return Err("Import map exceeds 128 entries".into());
                        }
                        for (key, value) in imports {
                            let target = if value.is_null() {
                                None
                            } else {
                                let target = value
                                    .as_str()
                                    .ok_or("Import map target must be URL or null")?;
                                let target = checked_url(
                                    base.join(target).map_err(|e| e.to_string())?.as_str(),
                                )?;
                                if key.ends_with('/') && !target.ends_with('/') {
                                    return Err(
                                        "Import map prefix target needs trailing slash".into()
                                    );
                                }
                                Some(target)
                            };
                            self.imports.entry(mapping_key(&base,key)?).or_insert(target);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

enum State {
    Queued,
    Fetching(u32, Instant),
    Ready {
        bytes: Arc<[u8]>,
        dependencies: Option<Vec<String>>,
    },
    Failed(String),
}
#[derive(Clone, Debug)]
pub(crate) struct Event {
    pub url: String,
    pub status: &'static str,
    pub detail: String,
}
pub(crate) enum Prepared<T> {
    Pending,
    Ready(T),
    Failed(String),
}
pub(crate) struct Loader {
    policy: Rc<RefCell<Policy>>,
    entries: HashMap<String, State>,
    inline: HashMap<String, Result<Vec<String>, String>>,
    imports: Vec<ImportRequest>,
    pub events: Vec<Event>,
    bytes: usize,
    parse_allowance: usize,
    wait_start: Option<Instant>,
    waited: Duration,
    closed: bool,
}
impl Loader {
    pub(crate) fn new(
        engine: &mut Engine,
        page: &str,
        document: Option<&rp5::Document>,
    ) -> Result<Self, String> {
        checked_url(page)?;
        let mut policy = Policy {
            page: page.into(),
            ..Default::default()
        };
        if let Some(doc) = document {
            policy.document_maps(doc, page)?;
        }
        let policy = Rc::new(RefCell::new(policy));
        let resolver = policy.clone();
        engine.install_modules(move |base, name| resolver.borrow_mut().resolve(base, name))?;
        Ok(Self {
            policy,
            entries: HashMap::new(),
            inline: HashMap::new(),
            imports: Vec::new(),
            events: Vec::new(),
            bytes: 0,
            parse_allowance: 1,
            wait_start: None,
            waited: Duration::ZERO,
            closed: false,
        })
    }
    pub(crate) fn waited(&self) -> Duration {
        self.waited + self.wait_start.map_or(Duration::ZERO, |t| t.elapsed())
    }
    pub(crate) fn pending(&self) -> bool {
        !self.imports.is_empty()
            || self
                .entries
                .values()
                .any(|s| matches!(s, State::Queued | State::Fetching(..)))
    }
    fn request(&mut self, url: &str) -> Result<(), String> {
        if self.entries.contains_key(url) {
            return Ok(());
        }
        if self.closed || self.entries.len() >= MAX_FILES {
            return Err("Module resource count/wait budget exhausted".into());
        }
        self.entries.insert(url.into(), State::Queued);
        if self.wait_start.is_none() {
            self.wait_start = Some(Instant::now());
        }
        Ok(())
    }
    fn fail(&mut self, url: String, error: String) {
        self.events.push(Event {
            url: url.clone(),
            status: "failed",
            detail: error.clone(),
        });
        self.entries.insert(url, State::Failed(error));
    }
    pub(crate) fn poll(&mut self, engine: &mut Engine) {
        self.parse_allowance = 1;
        if self.waited() >= MAX_WAIT && !self.closed {
            self.close();
        }
        let inflight = self
            .entries
            .iter()
            .filter_map(|(u, s)| {
                if let State::Fetching(id, start) = s {
                    Some((u.clone(), *id, *start))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for (url, id, start) in inflight {
            let result = if start.elapsed() >= FETCH_WAIT {
                Err("Resource download timed out".into())
            } else {
                match netfs::fetch_bytes_result_len(id) {
                    Err(-8) => continue,
                    Err(code) => Err(format!("Resource download failed: {code}")),
                    Ok(length)
                        if length > MAX_FILE || self.bytes.saturating_add(length) > MAX_TOTAL =>
                    {
                        Err("Resource byte budget exceeded".into())
                    }
                    Ok(_) => netfs::fetch_bytes_read(id)
                        .map_err(|code| format!("Resource read failed: {code}")),
                }
            };
            let _ = netfs::fetch_bytes_discard(id);
            match result {
                Ok(bytes) => {
                    if bytes.len() > MAX_FILE || self.bytes.saturating_add(bytes.len()) > MAX_TOTAL
                    {
                        self.fail(url, "Resource byte budget exceeded".into());
                        continue;
                    }
                    self.bytes += bytes.len();
                    self.events.push(Event {
                        url: url.clone(),
                        status: "downloaded",
                        detail: format!("{} bytes", bytes.len()),
                    });
                    self.entries.insert(
                        url,
                        State::Ready {
                            bytes: bytes.into(),
                            dependencies: None,
                        },
                    );
                }
                Err(e) => self.fail(url, e),
            }
        }
        let active = self
            .entries
            .values()
            .filter(|s| matches!(s, State::Fetching(..)))
            .count();
        let queued = self
            .entries
            .iter()
            .filter_map(|(url, state)| matches!(state, State::Queued).then_some(url.clone()))
            .take(4usize.saturating_sub(active))
            .collect::<Vec<_>>();
        for url in queued {
            match netfs::fetch_bytes_happy_eyeballs(url.as_bytes()) {
                Ok(id) => {
                    self.events.push(Event {
                        url: url.clone(),
                        status: "requested",
                        detail: String::new(),
                    });
                    self.entries
                        .insert(url, State::Fetching(id, Instant::now()));
                }
                Err(code) => self.fail(url, format!("Resource download start failed: {code}")),
            }
        }
        if !self
            .entries
            .values()
            .any(|s| matches!(s, State::Queued | State::Fetching(..)))
        {
            if let Some(start) = self.wait_start.take() {
                self.waited += start.elapsed();
            }
        }
        self.imports.extend(engine.take_import_requests());
        let pending = std::mem::take(&mut self.imports);
        for request in pending {
            let root = self
                .policy
                .borrow_mut()
                .resolve(&request.base, &request.specifier);
            let state = match root {
                Ok(root) => self.module(engine, &root),
                Err(e) => Prepared::Failed(e),
            };
            let result = match state {
                Prepared::Pending => {
                    self.imports.push(request);
                    continue;
                }
                Prepared::Ready(_) => engine.complete_import(request.id, None),
                Prepared::Failed(error) => {
                    self.events.push(Event {
                        url: request.specifier.clone(),
                        status: "import-failed",
                        detail: error.clone(),
                    });
                    engine.complete_import(request.id, Some(&error))
                }
            };
            if let Err(e) = result {
                self.events.push(Event {
                    url: request.specifier,
                    status: "import-failed",
                    detail: e,
                });
            }
        }
    }
    fn graph(
        &mut self,
        engine: &mut Engine,
        url: &str,
        seen: &mut HashSet<String>,
    ) -> Prepared<()> {
        let url = self.policy.borrow().canonical(url);
        if !seen.insert(url.clone()) {
            return Prepared::Ready(());
        }
        if let Err(e) = self.request(&url) {
            return Prepared::Failed(e);
        }
        let (bytes, deps) = match self.entries.get(&url).unwrap() {
            State::Queued | State::Fetching(..) => return Prepared::Pending,
            State::Failed(e) => return Prepared::Failed(format!("{url}: {e}")),
            State::Ready {
                bytes,
                dependencies,
            } => (bytes.clone(), dependencies.clone()),
        };
        let deps = if let Some(deps) = deps {
            deps
        } else {
            if self.parse_allowance == 0 {
                return Prepared::Pending;
            }
            self.parse_allowance -= 1;
            let raw = match engine.module_dependencies(&bytes, &url) {
                Ok(deps) => deps,
                Err(e) => {
                    self.fail(url.clone(), e.clone());
                    return Prepared::Failed(e);
                }
            };
            let mut deps = Vec::new();
            for name in raw {
                match self.policy.borrow_mut().resolve(&url, &name) {
                    Ok(dep) => deps.push(dep),
                    Err(e) => return Prepared::Failed(e),
                }
            }
            if let Err(e) = engine.add_module(&url, &bytes) {
                return Prepared::Failed(e);
            }
            self.entries.insert(
                url.clone(),
                State::Ready {
                    bytes,
                    dependencies: Some(deps.clone()),
                },
            );
            deps
        };
        let mut pending = false;
        for dependency in deps {
            match self.graph(engine, &dependency, seen) {
                Prepared::Ready(()) => {}
                Prepared::Pending => pending = true,
                Prepared::Failed(e) => return Prepared::Failed(e),
            }
        }
        if pending {
            Prepared::Pending
        } else {
            Prepared::Ready(())
        }
    }
    pub(crate) fn module(&mut self, engine: &mut Engine, url: &str) -> Prepared<String> {
        match self.graph(engine, url, &mut HashSet::new()) {
            Prepared::Ready(()) => Prepared::Ready(self.policy.borrow().canonical(url)),
            Prepared::Pending => Prepared::Pending,
            Prepared::Failed(error) => {
                let fallback = self.policy.borrow_mut().switch(url);
                if let Some(fallback) = fallback {
                    self.events.push(Event {
                        url: url.into(),
                        status: "fallback",
                        detail: fallback.clone(),
                    });
                    match self.request(&fallback) {
                        Ok(()) => Prepared::Pending,
                        Err(e) => Prepared::Failed(e),
                    }
                } else {
                    Prepared::Failed(error)
                }
            }
        }
    }
    pub(crate) fn inline(
        &mut self,
        engine: &mut Engine,
        origin: &str,
        bytes: &[u8],
    ) -> Prepared<()> {
        if !self.inline.contains_key(origin) {
            if self.parse_allowance == 0 {
                return Prepared::Pending;
            }
            self.parse_allowance -= 1;
            let deps = engine.module_dependencies(bytes, origin).and_then(|names| {
                names
                    .into_iter()
                    .map(|name| self.policy.borrow_mut().resolve(origin, &name))
                    .collect()
            });
            self.inline.insert(origin.into(), deps);
        }
        let deps = match self.inline.get(origin).unwrap() {
            Ok(deps) => deps.clone(),
            Err(e) => return Prepared::Failed(e.clone()),
        };
        let mut pending = false;
        for dep in deps {
            match self.module(engine, &dep) {
                Prepared::Ready(_) => {}
                Prepared::Pending => pending = true,
                Prepared::Failed(e) => return Prepared::Failed(e),
            }
        }
        if pending {
            Prepared::Pending
        } else {
            Prepared::Ready(())
        }
    }
    pub(crate) fn source(&mut self, url: &str) -> Prepared<Arc<[u8]>> {
        let url = match checked_url(url) {
            Ok(url) => url,
            Err(e) => return Prepared::Failed(e),
        };
        if let Err(e) = self.request(&url) {
            return Prepared::Failed(e);
        }
        match self.entries.get(&url).unwrap() {
            State::Ready { bytes, .. } => Prepared::Ready(bytes.clone()),
            State::Failed(e) => Prepared::Failed(e.clone()),
            _ => Prepared::Pending,
        }
    }
    pub(crate) fn module_source(
        &mut self,
        engine: &mut Engine,
        url: &str,
    ) -> Prepared<(String, Arc<[u8]>)> {
        let url = match self.policy.borrow_mut().resolve(url, url) {
            Ok(url) => url,
            Err(e) => return Prepared::Failed(e),
        };
        match self.module(engine, &url) {
            Prepared::Pending => Prepared::Pending,
            Prepared::Failed(e) => Prepared::Failed(e),
            Prepared::Ready(url) => match self.source(&url) {
                Prepared::Ready(bytes) => Prepared::Ready((url, bytes)),
                Prepared::Pending => Prepared::Pending,
                Prepared::Failed(e) => Prepared::Failed(e),
            },
        }
    }
    pub(crate) fn close(&mut self) {
        self.closed = true;
        let unfinished = self
            .entries
            .iter()
            .filter_map(|(u, s)| match s {
                State::Queued => Some((u.clone(), None)),
                State::Fetching(id, _) => Some((u.clone(), Some(*id))),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (url, id) in unfinished {
            if let Some(id) = id {
                let _ = netfs::fetch_bytes_discard(id);
            }
            self.fail(url, "Resource wait budget exhausted".into());
        }
        if let Some(start) = self.wait_start.take() {
            self.waited += start.elapsed();
        }
    }
}
impl Drop for Loader {
    fn drop(&mut self) {
        self.close();
    }
}
