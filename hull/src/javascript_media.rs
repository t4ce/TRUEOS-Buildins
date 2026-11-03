//! One URL handoff slot per navigation. No decoder, network or pixel API in JS.
#[derive(Clone)]
pub(crate) struct Request {
    pub id: u32,
    pub node: usize,
    pub url: String,
}
#[derive(Default)]
pub(crate) struct Media {
    next: u32,
    pub inspection_navigation: std::collections::VecDeque<String>,
    pub pending: Option<Request>,
    pub receipt: Option<(u32, i32)>,
}
pub(crate) type SharedMedia = std::rc::Rc<std::cell::RefCell<Media>>;
impl Media {
    pub fn request(&mut self, node: usize, url: String) -> u32 {
        self.next = self.next.checked_add(1).unwrap_or(1);
        let id = self.next;
        self.pending = Some(Request { id, node, url });
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
    let Ok(url)=url::Url::parse(address) else { return false; };
    matches!(url.host_str(),Some("www.google.com"|"www.recaptcha.net"|"www.gstatic.com"))
        && url.path().starts_with("/recaptcha/")
}
