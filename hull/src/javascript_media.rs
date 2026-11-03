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
