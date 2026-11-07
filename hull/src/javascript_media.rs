//! One URL handoff slot per navigation. No decoder, network or pixel API in JS.
use alloc::{string::{String, ToString}, format};
#[derive(Clone)]
pub(crate) struct Request {
    pub id: u32,
    pub node: usize,
    pub url: String,
    pub referrer: Option<String>,
}
#[derive(Default)]
pub(crate) struct Media {
    next: u32,
    pub inspection_navigation: alloc::collections::VecDeque<String>,
    pub pending: Option<Request>,
    pub receipt: Option<(u32, i32)>,
}
pub(crate) type SharedMedia = alloc::rc::Rc<core::cell::RefCell<Media>>;
impl Media {
    pub fn has_request(&self) -> bool {
        self.next != 0
    }
    pub fn request(&mut self, node: usize, url: String, document_url: &str) -> u32 {
        self.next = self.next.checked_add(1).unwrap_or(1);
        let id = self.next;
        self.pending = Some(Request { id, node, referrer: media_referrer(document_url, &url), url });
        self.receipt = None;
        id
    }
}

/// Known player entry points are adapters, not browser UI implementations.
pub(crate) fn player_adapter(address: &str) -> Option<&'static [u8]> {
    let url = url::Url::parse(address).ok()?;
    (crate::resource_url::allowed(&url) && url.path().ends_with("/kt_player.js"))
        .then_some(include_bytes!("javascript_player_kt.js").as_slice())
}
pub(crate) fn ignored_script(address: &str) -> bool {
    let Ok(url) = url::Url::parse(address) else {
        return false;
    };
    matches!(
        url.host_str(),
        Some("www.google.com" | "www.recaptcha.net" | "www.gstatic.com")
    ) && url.path().starts_with("/recaptcha/")
}

/// Default strict-origin-when-cross-origin policy; never send fragments or credentials.
pub(crate) fn media_referrer(document: &str, target: &str) -> Option<String> {
    let mut source=url::Url::parse(document).ok()?;
    let target=url::Url::parse(target).ok()?;
    if source.scheme()!="https" || target.scheme()!="https" || !source.username().is_empty() || source.password().is_some() {return None;}
    source.set_fragment(None);
    let result=if source.origin()==target.origin() {source.to_string()} else {format!("{}/",source.origin().ascii_serialization())};
    (result.len()<=3072).then_some(result)
}
