//! Finite per-realm timer deadlines; callbacks remain owned by QuickJS.
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};
const MAX_TIMERS: usize = 256;
#[derive(Default)]
pub(crate) struct Timers {
    next: u32,
    pending: Vec<(u32, Instant)>,
}
impl Timers {
    pub fn take_due(&mut self) -> Option<u32> {
        let now = Instant::now();
        let (index, _) = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, (_, at))| *at <= now)
            .min_by_key(|(_, (_, at))| *at)?;
        Some(self.pending.remove(index).0)
    }
}
pub(crate) type SharedTimers = Rc<RefCell<Timers>>;
pub(crate) fn call(state: &SharedTimers, args: &[Value]) -> Value {
    let mut state = state.borrow_mut();
    if args.first().and_then(Value::as_str) == Some("clear") {
        if let Some(id) = args.get(1).and_then(Value::as_u64) {
            state.pending.retain(|(key, _)| u64::from(*key) != id);
        }
        Value::Null
    } else if state.pending.len() >= MAX_TIMERS {
        json!(0)
    } else {
        state.next = if state.next >= i32::MAX as u32 {
            1
        } else {
            state.next + 1
        };
        while state.pending.iter().any(|(id, _)| *id == state.next) {
            state.next += 1;
        }
        let id = state.next;
        let delay = args
            .get(1)
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(i32::MAX as u64);
        state
            .pending
            .push((id, Instant::now() + Duration::from_millis(delay)));
        json!(id)
    }
}
