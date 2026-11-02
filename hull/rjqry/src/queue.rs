//! Native queue bookkeeping translated from jQuery 4.0.0's
//! `jquery-main/src/queue.js` (MIT).
//!
//! A queue belongs to one host-selected owner and type. Work items remain typed
//! host values; this module does not create a scheduler. The host executes the
//! value returned by `dequeue`, and calls `dequeue` again for jQuery's `next()`.
//! Owner lookup, timers, and the collection-level Deferred adapter belong to
//! their respective host/data layers.

use crate::callbacks::{CallbackList, CallbackOptions};
use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

/// A host effect's stop callback. `true` requests its final state (`gotoEnd`).
pub type StopHook = Rc<dyn Fn(bool)>;

/// Hooks for one queue lifetime. Empty callbacks are `once memory`, as upstream.
#[derive(Clone)]
pub struct QueueHooks {
    empty: CallbackList<(), ()>,
    stop: Rc<RefCell<Option<StopHook>>>,
}

impl Default for QueueHooks {
    fn default() -> Self {
        Self {
            empty: CallbackList::new(CallbackOptions {
                once: true,
                memory: true,
                ..CallbackOptions::default()
            }),
            stop: Rc::new(RefCell::new(None)),
        }
    }
}

impl QueueHooks {
    pub fn empty(&self) -> CallbackList<(), ()> {
        self.empty.clone()
    }

    pub fn set_stop(&self, callback: impl Fn(bool) + 'static) {
        *self.stop.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn has_stop(&self) -> bool {
        self.stop.borrow().is_some()
    }

    /// Remove a stop callback without invoking it, just like deleting
    /// `hooks.stop`. A host may call the returned function outside any borrow.
    pub fn take_stop(&self) -> Option<StopHook> {
        self.stop.borrow_mut().take()
    }
}

struct State<T> {
    pending: VecDeque<T>,
    // The source represents this as the string "inprogress" at index zero.
    in_progress: bool,
    hooks: Option<QueueHooks>,
}

/// One named queue. Clones share its items and hooks.
///
/// `push`, `replace`, and `clear` correspond to the static `jQuery.queue`
/// storage operations. They do not invoke a work item. `enqueue` additionally
/// applies the collection setter's automatic start rule for an `fx` queue.
pub struct Queue<T> {
    name: String,
    state: Rc<RefCell<State<T>>>,
}

impl<T> Clone for Queue<T> {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            state: self.state.clone(),
        }
    }
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self::new("fx")
    }
}

impl<T> Queue<T> {
    /// An empty type name selects `fx`, matching `(type || "fx")` upstream.
    pub fn new(name: &str) -> Self {
        Self {
            name: if name.is_empty() { "fx" } else { name }.to_string(),
            state: Rc::new(RefCell::new(State {
                pending: VecDeque::new(),
                in_progress: false,
                hooks: None,
            })),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_fx(&self) -> bool {
        self.name == "fx"
    }

    /// The upstream queue length, including the fx progress sentinel.
    pub fn len(&self) -> usize {
        let state = self.state.borrow();
        state.pending.len() + usize::from(state.in_progress)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The number of host work items waiting after the current fx work item.
    pub fn pending_len(&self) -> usize {
        self.state.borrow().pending.len()
    }

    /// Whether the `fx` sentinel is present. Custom queues have no sentinel,
    /// even while a previously dequeued item is executing in the host.
    pub fn running(&self) -> bool {
        self.state.borrow().in_progress
    }

    pub fn push(&self, item: T) -> &Self {
        self.state.borrow_mut().pending.push_back(item);
        self
    }

    /// Replace the entire array. Like an upstream array assignment, this also
    /// removes the progress sentinel, without stopping already dispatched work.
    pub fn replace(&self, items: impl IntoIterator<Item = T>) -> &Self {
        let items: VecDeque<_> = items.into_iter().collect();
        let mut state = self.state.borrow_mut();
        state.pending = items;
        state.in_progress = false;
        self
    }

    /// Empty storage without running hooks. Call `dequeue` to observe emptiness.
    pub fn clear(&self) -> &Self {
        self.replace(core::iter::empty())
    }

    /// Add an item and return it if the collection setter would start it now.
    /// The caller invokes a returned item and supplies its continuation/hook
    /// access; custom queues remain pending until explicitly dequeued.
    pub fn enqueue(&self, item: T) -> Option<T> {
        self.push(item);
        self.hooks();
        if self.is_fx() && !self.running() {
            self.dequeue()
        } else {
            None
        }
    }

    /// The collection setter's replacement form, including fx automatic start.
    pub fn replace_and_start(&self, items: impl IntoIterator<Item = T>) -> Option<T> {
        self.replace(items);
        self.hooks();
        if self.is_fx() && !self.running() {
            self.dequeue()
        } else {
            None
        }
    }

    /// Equivalent to the collection `clearQueue` setter. This does not cancel
    /// dispatched work. For `fx`, the empty replacement is dequeued immediately.
    pub fn clear_queue(&self) -> &Self {
        self.replace_and_start(core::iter::empty());
        self
    }

    /// Return/create hooks for this lifetime. An empty firing removes the
    /// queue's reference before user callbacks, so a reentrant enqueue obtains
    /// fresh hooks while holders of the old hooks retain their memory replay.
    pub fn hooks(&self) -> QueueHooks {
        self.state
            .borrow_mut()
            .hooks
            .get_or_insert_with(QueueHooks::default)
            .clone()
    }

    /// Observe existing hooks without creating a pending lifetime. This is the
    /// queue-side operation used by an aggregate queue promise.
    pub fn existing_hooks(&self) -> Option<QueueHooks> {
        self.state.borrow().hooks.clone()
    }

    pub fn empty_callbacks(&self) -> CallbackList<(), ()> {
        self.hooks().empty()
    }

    /// Dispatch one work item, clearing the previous stop hook. A final call
    /// after the last work item is the `next()` that fires empty hooks.
    ///
    /// Work is returned rather than invoked so the host cannot run while this
    /// module holds a mutable state borrow. Empty callbacks likewise run after
    /// dropping that borrow and may reentrantly enqueue more work.
    pub fn dequeue(&self) -> Option<T> {
        let (item, hooks, empty) = {
            let mut state = self.state.borrow_mut();
            // Removing the sentinel reduces startLength by one upstream; the
            // resulting value is exactly the number of pending work items.
            let start_length = state.pending.len();
            state.in_progress = false;
            let item = state.pending.pop_front();
            let hooks = state.hooks.get_or_insert_with(QueueHooks::default).clone();
            if item.is_some() {
                state.in_progress = self.is_fx();
            }
            if start_length == 0 {
                // The first upstream empty callback removes queue and hooks
                // from dataPriv before any user empty callback can run.
                state.hooks = None;
            }
            (item, hooks, start_length == 0)
        };
        if item.is_some() {
            hooks.take_stop();
        }
        if empty {
            hooks.empty.fire(());
        }
        item
    }

    /// Snapshot waiting host values, excluding the private fx sentinel.
    pub fn pending(&self) -> Vec<T>
    where
        T: Clone,
    {
        self.state.borrow().pending.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callbacks::Callback;
    use alloc::vec;
    use core::cell::Cell;

    #[test]
    fn custom_queue_is_ordered_and_explicitly_started() {
        let queue = Queue::new("custom");
        assert_eq!(queue.enqueue(1), None);
        queue.push(2).push(3);
        assert_eq!(queue.pending(), vec![1, 2, 3]);
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.dequeue(), Some(1));
        assert_eq!(queue.len(), 2);
        assert!(!queue.running());
        assert_eq!(queue.dequeue(), Some(2));
        assert_eq!(queue.dequeue(), Some(3));
        assert_eq!(queue.dequeue(), None);
        assert!(queue.is_empty());
    }

    #[test]
    fn fx_auto_start_uses_sentinel_until_next() {
        let queue = Queue::new("");
        assert_eq!(queue.name(), "fx");
        assert_eq!(queue.enqueue(1), Some(1));
        assert!(queue.running());
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.len(), 1);
        assert!(!queue.is_empty());
        assert_eq!(queue.enqueue(2), None);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.dequeue(), Some(2));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.dequeue(), None);
        assert!(!queue.running());
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn static_storage_does_not_start_fx_work() {
        let queue = Queue::default();
        queue.push(1).push(2);
        assert_eq!(queue.pending_len(), 2);
        assert!(!queue.running());
        assert!(queue.existing_hooks().is_none());
        assert_eq!(queue.dequeue(), Some(1));
        assert!(queue.running());
        assert!(queue.existing_hooks().is_some());
    }

    #[test]
    fn empty_hook_requires_final_next_and_replays_for_late_listeners() {
        for name in ["fx", "custom"] {
            let queue = Queue::new(name);
            let count = Rc::new(Cell::new(0));
            let empty = queue.empty_callbacks();
            let callback = {
                let count = count.clone();
                Callback::new(move |_, _| {
                    count.set(count.get() + 1);
                    true
                })
            };
            empty.add(callback.clone());
            queue.push(1);
            assert_eq!(queue.dequeue(), Some(1));
            assert_eq!(count.get(), 0);
            assert_eq!(queue.dequeue(), None);
            assert_eq!(count.get(), 1);
            assert!(queue.existing_hooks().is_none());
            empty.add(callback);
            assert_eq!(count.get(), 2);
            assert_eq!(queue.dequeue(), None);
            assert_eq!(count.get(), 2, "old hooks never fire twice");
        }
    }

    #[test]
    fn stop_hook_is_cleared_at_next_dispatch_without_invocation() {
        let queue = Queue::default();
        queue.push(1).push(2);
        queue.dequeue();
        let hooks = queue.hooks();
        let stopped = Rc::new(Cell::new(false));
        hooks.set_stop({
            let stopped = stopped.clone();
            move |_| stopped.set(true)
        });
        assert!(hooks.has_stop());
        queue.dequeue();
        assert!(!hooks.has_stop());
        assert!(!stopped.get());
        hooks.set_stop({
            let stopped = stopped.clone();
            move |goto_end| stopped.set(goto_end)
        });
        hooks.take_stop().unwrap()(true);
        assert!(stopped.get());
        assert!(!hooks.has_stop());
    }

    #[test]
    fn replacement_and_clear_remove_sentinel_but_do_not_cancel_dispatched_work() {
        let queue = Queue::default();
        let dispatched = queue.enqueue(1);
        assert_eq!(dispatched, Some(1));
        queue.push(2).replace([3, 4]);
        assert!(!queue.running());
        assert_eq!(queue.pending(), vec![3, 4]);
        assert_eq!(queue.dequeue(), Some(3));
        queue.clear();
        assert!(!queue.running());
        assert!(queue.is_empty());
        assert_eq!(dispatched, Some(1));
    }

    #[test]
    fn clear_queue_immediately_dequeues_fx_only() {
        for name in ["fx", "custom"] {
            let queue = Queue::new(name);
            let fired = Rc::new(Cell::new(false));
            queue.empty_callbacks().add({
                let fired = fired.clone();
                Callback::new(move |_, _| {
                    fired.set(true);
                    true
                })
            });
            queue.push(1).clear_queue();
            assert!(queue.is_empty());
            assert_eq!(fired.get(), name == "fx");
            queue.dequeue();
            assert!(fired.get());
        }
    }

    #[test]
    fn empty_callback_can_enqueue_into_fresh_hooks_without_borrow_conflicts() {
        let queue = Queue::new("custom");
        let old_empty = queue.empty_callbacks();
        let fresh_fired = Rc::new(Cell::new(false));
        old_empty.add({
            let queue = queue.clone();
            let fresh_fired = fresh_fired.clone();
            Callback::new(move |_, _| {
                assert!(queue.existing_hooks().is_none());
                queue.push(2);
                queue.empty_callbacks().add({
                    let fresh_fired = fresh_fired.clone();
                    Callback::new(move |_, _| {
                        fresh_fired.set(true);
                        true
                    })
                });
                true
            })
        });
        queue.push(1);
        assert_eq!(queue.dequeue(), Some(1));
        assert_eq!(queue.dequeue(), None);
        assert_eq!(queue.pending(), vec![2]);
        assert!(old_empty.fired());
        assert!(!fresh_fired.get());
        assert_eq!(queue.dequeue(), Some(2));
        assert_eq!(queue.dequeue(), None);
        assert!(fresh_fired.get());
    }

    #[test]
    fn cloned_handles_share_storage_and_hooks() {
        let queue = Queue::new("custom");
        let clone = queue.clone();
        queue.push(1);
        assert_eq!(clone.dequeue(), Some(1));
        assert!(queue.is_empty());
        let empty = queue.empty_callbacks();
        assert!(!empty.fired());
        clone.dequeue();
        assert!(empty.fired());
    }
}
