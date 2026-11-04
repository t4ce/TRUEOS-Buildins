//! Deterministic local download tickets. Never opens a socket.
use std::{cell::RefCell, collections::HashMap};
#[derive(Default)]
struct Network {
    resources: HashMap<String, Vec<u8>>,
    delays: HashMap<String, usize>,
    tickets: HashMap<u32, String>,
    requested: Vec<String>,
    referrers: Vec<(String, String)>,
    discarded: Vec<String>,
    next: u32,
}
thread_local! { static NETWORK: RefCell<std::sync::Arc<std::sync::Mutex<Network>>> = RefCell::new(Default::default()); }
struct NetworkSlot;
static NET: NetworkSlot = NetworkSlot;
impl NetworkSlot {
    fn with<R>(&self, f: impl FnOnce(&std::sync::Mutex<Network>) -> R) -> R {
        NETWORK.with(|slot| f(&slot.borrow()))
    }
}
pub(crate) fn inherit() -> impl FnOnce() + Send + 'static {
    let shared = NETWORK.with(|slot| slot.borrow().clone());
    move || NETWORK.with(|slot| *slot.borrow_mut() = shared)
}
pub fn install(resources: impl IntoIterator<Item = (String, Vec<u8>)>) {
    NETWORK.with(|slot| {
        *slot.borrow_mut() = std::sync::Arc::new(std::sync::Mutex::new(Network {
            resources: resources.into_iter().collect(),
            ..Default::default()
        }));
    });
}
pub fn delay(url: &str, polls: usize) {
    NET.with(|n| {
        n.lock().unwrap().delays.insert(url.into(), polls);
    });
}
pub fn requested() -> Vec<String> {
    NET.with(|n| n.lock().unwrap().requested.clone())
}
pub fn requested_referrers() -> Vec<(String, String)> {
    NET.with(|n| n.lock().unwrap().referrers.clone())
}
pub fn active() -> usize {
    NET.with(|n| n.lock().unwrap().tickets.len())
}
pub fn discarded() -> Vec<String> {
    NET.with(|n| n.lock().unwrap().discarded.clone())
}
pub fn fetch_bytes_happy_eyeballs(url: &[u8]) -> Result<u32, i32> {
    let url = std::str::from_utf8(url).map_err(|_| -1)?.to_owned();
    NET.with(|n| {
        let mut n = n.lock().unwrap();
        n.next += 1;
        let id = n.next;
        n.requested.push(url.clone());
        n.tickets.insert(id, url);
        Ok(id)
    })
}
pub fn fetch_bytes_with_referrer(url: &[u8], referrer: &[u8]) -> Result<u32, i32> {
    let referrer = std::str::from_utf8(referrer).map_err(|_| -1)?.to_owned();
    let id = fetch_bytes_happy_eyeballs(url)?;
    NET.with(|n| {
        n.lock().unwrap().referrers.push((String::from_utf8_lossy(url).into_owned(), referrer));
    });
    Ok(id)
}
pub fn fetch_bytes_result_len(id: u32) -> Result<usize, i32> {
    NET.with(|n| {
        let mut n = n.lock().unwrap();
        let url = n.tickets.get(&id).cloned().ok_or(-1)?;
        if let Some(delay) = n.delays.get_mut(&url) {
            if *delay > 0 {
                *delay -= 1;
                return Err(-8);
            }
        }
        n.resources.get(&url).map(Vec::len).ok_or(-404)
    })
}
pub fn fetch_bytes_read(id: u32) -> Result<Vec<u8>, i32> {
    NET.with(|n| {
        let n = n.lock().unwrap();
        n.tickets
            .get(&id)
            .and_then(|u| n.resources.get(u))
            .cloned()
            .ok_or(-404)
    })
}
pub fn fetch_bytes_discard(id: u32) -> i32 {
    NET.with(|n| {
        let mut n = n.lock().unwrap();
        if let Some(url) = n.tickets.remove(&id) {
            n.discarded.push(url);
        }
    });
    0
}
