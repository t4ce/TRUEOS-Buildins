//! Finite per-realm timer deadlines; callbacks remain owned by QuickJS.
use alloc::{vec::Vec};
use serde_json::{Value, json};
use alloc::rc::Rc;
use core::cell::RefCell;
use crate::platform::{Duration, Instant};
const MAX_TIMERS: usize = 256;
#[derive(Default)]
pub(crate) struct Timers {
    next: u32,
    pending: Vec<Timer>,
}
struct Timer {
    id: u32,
    at: Instant,
    repeating: bool,
    in_flight: bool,
}
impl Timers {
    /// Remaining wait for the actor, excluding a callback currently dispatched.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.pending
            .iter()
            .filter(|timer| !timer.in_flight)
            .map(|timer| timer.at.saturating_duration_since(Instant::now()))
            .min()
    }
    pub fn take_due(&mut self) -> Option<u32> {
        let now = Instant::now();
        let (index, _) = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, timer)| !timer.in_flight && timer.at <= now)
            .min_by_key(|(_, timer)| timer.at)?;
        if self.pending[index].repeating {
            self.pending[index].in_flight = true;
            Some(self.pending[index].id)
        } else {
            Some(self.pending.remove(index).id)
        }
    }
}
pub(crate) type SharedTimers = Rc<RefCell<Timers>>;
pub(crate) fn call(state: &SharedTimers, args: &[Value]) -> Value {
    let mut state = state.borrow_mut();
    if args.first().and_then(Value::as_str) == Some("clear") {
        if let Some(id) = args.get(1).and_then(Value::as_u64) {
            state.pending.retain(|timer| u64::from(timer.id) != id);
        }
        Value::Null
    } else if args.first().and_then(Value::as_str) == Some("rearm") {
        let id = args.get(1).and_then(Value::as_u64).unwrap_or(0);
        let delay = args
            .get(2)
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(i32::MAX as u64);
        if let Some(timer) = state
            .pending
            .iter_mut()
            .find(|timer| u64::from(timer.id) == id && timer.repeating && timer.in_flight)
        {
            timer.at = Instant::now() + Duration::from_millis(delay);
            timer.in_flight = false;
            json!(true)
        } else {
            json!(false)
        }
    } else if !matches!(
        args.first().and_then(Value::as_str),
        Some("set" | "interval")
    ) || state.pending.len() >= MAX_TIMERS
    {
        json!(0)
    } else {
        state.next = if state.next >= i32::MAX as u32 {
            1
        } else {
            state.next + 1
        };
        while state.pending.iter().any(|timer| timer.id == state.next) {
            state.next = if state.next >= i32::MAX as u32 {
                1
            } else {
                state.next + 1
            };
        }
        let id = state.next;
        let delay = args
            .get(1)
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(i32::MAX as u64);
        state.pending.push(Timer {
            id,
            at: Instant::now() + Duration::from_millis(delay),
            repeating: args.first().and_then(Value::as_str) == Some("interval"),
            in_flight: false,
        });
        json!(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeating_deadlines_are_rearmed_once_after_dispatch_and_clear_cannot_resurrect() {
        let state = Rc::new(RefCell::new(Timers::default()));
        let id = call(&state, &[json!("interval"), json!(0)])
            .as_u64()
            .unwrap();
        assert_eq!(state.borrow_mut().take_due(), Some(id as u32));
        assert_eq!(state.borrow_mut().take_due(), None);
        assert_eq!(state.borrow().next_deadline(), None);
        assert_eq!(
            call(&state, &[json!("rearm"), json!(id), json!(100)]),
            json!(true)
        );
        assert!(state.borrow().next_deadline().unwrap() > Duration::from_millis(50));
        state.borrow_mut().pending[0].at = Instant::now() - Duration::from_secs(1);
        assert_eq!(state.borrow_mut().take_due(), Some(id as u32));
        call(&state, &[json!("clear"), json!(id)]);
        assert_eq!(
            call(&state, &[json!("rearm"), json!(id), json!(0)]),
            json!(false)
        );
        assert_eq!(state.borrow().next_deadline(), None);
    }
    #[test]
    fn timeout_and_interval_share_ids_quota_and_deadline_order() {
        let state = Rc::new(RefCell::new(Timers::default()));
        let repeating = call(&state, &[json!("interval"), json!(0)]);
        let once = call(&state, &[json!("set"), json!(0)]);
        assert_ne!(repeating, once);
        assert_eq!(
            state.borrow_mut().take_due(),
            Some(repeating.as_u64().unwrap() as u32)
        );
        assert_eq!(
            state.borrow_mut().take_due(),
            Some(once.as_u64().unwrap() as u32)
        );
        for _ in 0..255 {
            assert_ne!(call(&state, &[json!("set"), json!(10000)]), json!(0));
        }
        assert_eq!(call(&state, &[json!("interval"), json!(0)]), json!(0));
        assert_eq!(state.borrow().pending.len(), 256);
    }
}
