//! Native callback-list state machine translated from jQuery 4.0.0's
//! `jquery-main/src/callbacks.js` (MIT).
//!
//! Callbacks receive a typed context and argument value instead of JavaScript's
//! `this` and variadic arguments. Cloning a callback preserves its identity;
//! constructing another callback creates a distinct identity. Cloning a list
//! shares the list, so a callback may add, remove, lock or fire that same list.

use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;

/// The four behavior flags accepted by `jQuery.Callbacks`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallbackOptions {
    pub once: bool,
    pub memory: bool,
    pub unique: bool,
    pub stop_on_false: bool,
}

impl CallbackOptions {
    /// Parse jQuery's space-separated flags. Unknown flags have no effect.
    pub fn parse(flags: &str) -> Self {
        let mut options = Self::default();
        // jQuery uses HTML whitespace, which does not include vertical tab.
        for flag in flags.split(|c| matches!(c, ' ' | '\t' | '\r' | '\n' | '\u{000c}')) {
            match flag {
                "once" => options.once = true,
                "memory" => options.memory = true,
                "unique" => options.unique = true,
                "stopOnFalse" => options.stop_on_false = true,
                _ => {}
            }
        }
        options
    }
}

impl From<&str> for CallbackOptions {
    fn from(flags: &str) -> Self {
        Self::parse(flags)
    }
}

/// Short spelling for callers translating the upstream `options` object.
pub type Options = CallbackOptions;

type CallbackFn<C, A> = dyn Fn(&C, &A) -> bool;

/// An identity-bearing callback. Only a `false` return value can stop a list.
pub struct Callback<C, A>(Rc<CallbackFn<C, A>>);

impl<C, A> Callback<C, A> {
    pub fn new(callback: impl Fn(&C, &A) -> bool + 'static) -> Self {
        Self(Rc::new(callback))
    }

    pub fn call(&self, context: &C, args: &A) -> bool {
        (self.0)(context, args)
    }

    pub fn same(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl<C, A> Clone for Callback<C, A> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<C, A> fmt::Debug for Callback<C, A> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Callback").finish_non_exhaustive()
    }
}

struct State<C, A> {
    options: CallbackOptions,
    // None is the upstream spent/disabled list (the empty string).
    list: Option<Vec<Callback<C, A>>>,
    queue: VecDeque<(C, A)>,
    memory: Option<(C, A)>,
    firing: bool,
    fired: bool,
    locked: bool,
    firing_index: isize,
}

/// A shared, synchronous, reentrant jQuery callback list.
///
/// User callbacks run without borrowing the list's state. As in the upstream
/// implementation, recursively firing a repeatable list queues another turn,
/// while callbacks appended during a turn run at the end of that same turn.
/// The host is responsible for bounding callback execution if needed.
pub struct CallbackList<C, A>(Rc<RefCell<State<C, A>>>);

impl<C, A> Clone for CallbackList<C, A> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<C: Clone, A: Clone> Default for CallbackList<C, A> {
    fn default() -> Self {
        Self::new(CallbackOptions::default())
    }
}

impl<C: Clone, A: Clone> CallbackList<C, A> {
    pub fn new(options: CallbackOptions) -> Self {
        Self(Rc::new(RefCell::new(State {
            options,
            list: Some(Vec::new()),
            queue: VecDeque::new(),
            memory: None,
            firing: false,
            fired: false,
            locked: false,
            firing_index: -1,
        })))
    }

    pub fn add(&self, callback: Callback<C, A>) -> &Self {
        self.add_many(core::iter::once(callback))
    }

    /// Add a flat sequence. Rust callers flatten nested collections themselves;
    /// non-function JavaScript values have no native callback representation.
    pub fn add_many(&self, callbacks: impl IntoIterator<Item = Callback<C, A>>) -> &Self {
        // Do not execute an arbitrary iterator while holding the state borrow.
        let callbacks: Vec<_> = callbacks.into_iter().collect();
        let replay = {
            let mut state = self.0.borrow_mut();
            let Some(list) = state.list.as_ref() else {
                return self;
            };
            let replay = state.memory.is_some() && !state.firing;
            if replay {
                state.firing_index = list.len() as isize - 1;
                let memory = state.memory.clone().expect("checked callback memory");
                state.queue.push_back(memory);
            }
            let unique = state.options.unique;
            let list = state.list.as_mut().expect("checked callback list");
            for callback in callbacks {
                if !unique || !list.iter().any(|existing| existing.same(&callback)) {
                    list.push(callback);
                }
            }
            replay
        };
        if replay {
            self.run();
        }
        self
    }

    /// Remove every occurrence of this callback, including during a firing.
    pub fn remove(&self, callback: &Callback<C, A>) -> &Self {
        let mut state = self.0.borrow_mut();
        let mut index = 0;
        while let Some(list) = state.list.as_mut() {
            if index >= list.len() {
                break;
            }
            if list[index].same(callback) {
                list.remove(index);
                if index as isize <= state.firing_index {
                    state.firing_index -= 1;
                }
            } else {
                index += 1;
            }
        }
        self
    }

    pub fn has(&self, callback: &Callback<C, A>) -> bool {
        self.0
            .borrow()
            .list
            .as_ref()
            .is_some_and(|list| list.iter().any(|existing| existing.same(callback)))
    }

    pub fn has_any(&self) -> bool {
        self.0
            .borrow()
            .list
            .as_ref()
            .is_some_and(|list| !list.is_empty())
    }

    pub fn len(&self) -> usize {
        self.0.borrow().list.as_ref().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        !self.has_any()
    }

    pub fn empty(&self) -> &Self {
        if let Some(list) = self.0.borrow_mut().list.as_mut() {
            list.clear();
        }
        self
    }

    /// Abort the current turn and pending turns and forget all callbacks/data.
    pub fn disable(&self) -> &Self {
        let mut state = self.0.borrow_mut();
        state.locked = true;
        state.queue.clear();
        state.list = None;
        state.memory = None;
        self
    }

    pub fn disabled(&self) -> bool {
        self.0.borrow().list.is_none()
    }

    /// Prevent further explicit fires and discard queued turns. The current
    /// turn continues, and retained memory may still replay to new callbacks.
    pub fn lock(&self) -> &Self {
        let mut state = self.0.borrow_mut();
        state.locked = true;
        state.queue.clear();
        if state.memory.is_none() && !state.firing {
            state.list = None;
            state.memory = None;
        }
        self
    }

    pub fn locked(&self) -> bool {
        self.0.borrow().locked
    }

    /// Queue one firing with owned context and arguments. `Clone` is the native
    /// argument-copy boundary; use owned containers for JavaScript-like copies.
    pub fn fire_with(&self, context: C, args: A) -> &Self {
        let run = {
            let mut state = self.0.borrow_mut();
            if state.locked {
                return self;
            }
            state.queue.push_back((context, args));
            !state.firing
        };
        if run {
            self.run();
        }
        self
    }

    pub fn fired(&self) -> bool {
        self.0.borrow().fired
    }

    fn run(&self) {
        {
            let mut state = self.0.borrow_mut();
            state.locked |= state.options.once;
            state.fired = true;
            state.firing = true;
        }

        loop {
            {
                let mut state = self.0.borrow_mut();
                let Some(memory) = state.queue.pop_front() else {
                    break;
                };
                state.memory = Some(memory);
            }
            loop {
                let next = {
                    let mut state = self.0.borrow_mut();
                    state.firing_index += 1;
                    state.list.as_ref().and_then(|list| {
                        list.get(state.firing_index as usize).map(|callback| {
                            (
                                callback.clone(),
                                state.memory.clone().expect("current callback arguments"),
                            )
                        })
                    })
                };
                let Some((callback, (context, args))) = next else {
                    break;
                };
                let returned = callback.call(&context, &args);
                let mut state = self.0.borrow_mut();
                if !returned && state.options.stop_on_false {
                    state.firing_index = state.list.as_ref().map_or(0, Vec::len) as isize;
                    state.memory = None;
                }
            }
            self.0.borrow_mut().firing_index = -1;
        }

        let mut state = self.0.borrow_mut();
        if !state.options.memory {
            state.memory = None;
        }
        state.firing = false;
        if state.locked {
            state.list = if state.memory.is_some() {
                Some(Vec::new())
            } else {
                None
            };
        }
    }
}

impl<A: Clone> CallbackList<(), A> {
    /// Fire without an application context.
    pub fn fire(&self, args: A) -> &Self {
        self.fire_with((), args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;
    use core::cell::Cell;

    fn output(log: &Rc<RefCell<String>>, text: &'static str) -> Callback<(), ()> {
        let log = log.clone();
        Callback::new(move |_, _| {
            log.borrow_mut().push_str(text);
            true
        })
    }

    #[test]
    fn parses_html_whitespace_and_ignores_unknown_flags() {
        assert_eq!(
            CallbackOptions::parse(" once\tmemory\nunique\rstopOnFalse\u{c}unknown "),
            CallbackOptions {
                once: true,
                memory: true,
                unique: true,
                stop_on_false: true
            }
        );
        assert_eq!(
            CallbackOptions::parse("once\u{b}memory"),
            CallbackOptions::default()
        );
    }

    #[test]
    fn identity_unique_and_remove_all_instances() {
        let list = CallbackList::default();
        let log = Rc::new(RefCell::new(String::new()));
        let a = output(&log, "a");
        let b = output(&log, "b");
        list.add_many([a.clone(), a.clone(), b.clone()]);
        assert!(list.has(&a));
        assert_eq!(list.len(), 3);
        list.remove(&a).fire(());
        assert!(!list.has(&a));
        assert_eq!(&*log.borrow(), "b");
        let list = CallbackList::new(CallbackOptions::parse("unique"));
        list.add_many([a.clone(), a.clone()]);
        assert_eq!(list.len(), 1);
        list.add(output(&log, "a"));
        assert_eq!(
            list.len(),
            2,
            "equal behavior does not imply equal identity"
        );
    }

    #[test]
    fn once_memory_context_and_late_callbacks() {
        let list = CallbackList::new(CallbackOptions::parse("once memory"));
        assert!(!list.fired());
        assert!(!list.locked());
        let log = Rc::new(RefCell::new(Vec::new()));
        let record = {
            let log = log.clone();
            Callback::new(move |context: &String, args: &Vec<i32>| {
                log.borrow_mut().push((context.clone(), args.clone()));
                true
            })
        };
        list.add(record.clone());
        list.fire_with(String::from("context"), vec![1, 2]);
        assert!(list.fired());
        assert!(list.locked());
        assert!(!list.disabled());
        assert!(!list.has_any());
        list.fire_with(String::from("ignored"), vec![3]);
        list.add(record);
        assert_eq!(
            &*log.borrow(),
            &vec![(String::from("context"), vec![1, 2]); 2]
        );
    }

    #[test]
    fn append_during_fire_and_memory_replay_only_new_callbacks() {
        let log = Rc::new(RefCell::new(String::new()));
        let list = CallbackList::new(CallbackOptions::parse("memory"));
        let c = output(&log, "c");
        let a = {
            let list = list.clone();
            let log = log.clone();
            Callback::new(move |_, _| {
                list.add(c.clone());
                log.borrow_mut().push('a');
                true
            })
        };
        list.add_many([a, output(&log, "b")]).fire(());
        assert_eq!(&*log.borrow(), "abc");
        list.add(output(&log, "d"));
        assert_eq!(&*log.borrow(), "abcd");
        list.fire(());
        assert_eq!(&*log.borrow(), "abcdabcdc");
    }

    #[test]
    fn remove_current_and_previous_callbacks_during_fire() {
        let list = CallbackList::default();
        let log = Rc::new(RefCell::new(String::new()));
        let a = output(&log, "a");
        let this = Rc::new(RefCell::new(None));
        let b = {
            let list = list.clone();
            let log = log.clone();
            let a = a.clone();
            let this = this.clone();
            Callback::new(move |_, _| {
                log.borrow_mut().push('b');
                list.remove(&a).remove(this.borrow().as_ref().unwrap());
                true
            })
        };
        *this.borrow_mut() = Some(b.clone());
        list.add_many([a, b, output(&log, "c")]).fire(());
        assert_eq!(&*log.borrow(), "abc");
        list.fire(());
        assert_eq!(&*log.borrow(), "abcc");
    }

    #[test]
    fn remove_future_callback_during_fire() {
        let list = CallbackList::default();
        let log = Rc::new(RefCell::new(String::new()));
        let b = output(&log, "b");
        let a = {
            let list = list.clone();
            let b = b.clone();
            Callback::new(move |_, _| {
                list.remove(&b);
                true
            })
        };
        list.add_many([a, b, output(&log, "c")]).fire(());
        assert_eq!(&*log.borrow(), "c");
    }

    #[test]
    fn reentrant_fire_is_queued_and_once_blocks_it() {
        for once in [false, true] {
            let list = CallbackList::new(CallbackOptions {
                once,
                ..CallbackOptions::default()
            });
            let log = Rc::new(RefCell::new(Vec::new()));
            let a = {
                let list = list.clone();
                let log = log.clone();
                Callback::new(move |_, arg: &i32| {
                    log.borrow_mut().push(('a', *arg));
                    if *arg == 1 {
                        list.fire(2);
                    }
                    true
                })
            };
            let b = {
                let log = log.clone();
                Callback::new(move |_, arg: &i32| {
                    log.borrow_mut().push(('b', *arg));
                    true
                })
            };
            list.add_many([a, b]).fire(1);
            let expected = if once {
                vec![('a', 1), ('b', 1)]
            } else {
                vec![('a', 1), ('b', 1), ('a', 2), ('b', 2)]
            };
            assert_eq!(*log.borrow(), expected);
        }
    }

    #[test]
    fn stop_on_false_clears_memory_and_allows_later_turns() {
        let list = CallbackList::new(CallbackOptions::parse("memory stopOnFalse"));
        let log = Rc::new(RefCell::new(String::new()));
        let stop = Callback::new(|_, _| false);
        list.add_many([output(&log, "a"), stop.clone(), output(&log, "b")])
            .fire(());
        list.add(output(&log, "c"));
        assert_eq!(&*log.borrow(), "a", "false prevents late memory replay");
        list.remove(&stop).fire(());
        assert_eq!(&*log.borrow(), "aabc");
        list.add(output(&log, "d"));
        assert_eq!(&*log.borrow(), "aabcd");
    }

    #[test]
    fn once_stop_on_false_becomes_disabled() {
        let list = CallbackList::<(), ()>::new(CallbackOptions::parse("once memory stopOnFalse"));
        list.add(Callback::new(|_, _| false)).fire(());
        assert!(list.disabled());
        assert!(list.locked());
        assert!(list.fired());
    }

    #[test]
    fn empty_and_disable_abort_current_turn() {
        for disable in [false, true] {
            let list = CallbackList::default();
            let log = Rc::new(RefCell::new(String::new()));
            let control = {
                let list = list.clone();
                Callback::new(move |_, _| {
                    if disable {
                        list.disable();
                    } else {
                        list.empty();
                    }
                    true
                })
            };
            list.add_many([control, output(&log, "x")]).fire(());
            assert_eq!(&*log.borrow(), "");
            assert_eq!(list.disabled(), disable);
            list.empty().add(output(&log, "y")).fire(());
            assert_eq!(&*log.borrow(), if disable { "" } else { "y" });
        }
    }

    #[test]
    fn lock_during_fire_continues_current_turn_discards_queue_and_retains_memory() {
        let list = CallbackList::new(CallbackOptions::parse("memory"));
        let log = Rc::new(RefCell::new(String::new()));
        let control = {
            let list = list.clone();
            Callback::new(move |_, _| {
                list.fire(()).lock();
                true
            })
        };
        list.add_many([control, output(&log, "a")]).fire(());
        assert_eq!(&*log.borrow(), "a");
        assert!(list.locked());
        assert!(!list.disabled());
        list.add(output(&log, "b")).fire(());
        assert_eq!(&*log.borrow(), "ab");
        assert!(
            list.locked(),
            "memory replay cannot unlock a list (gh-3469)"
        );
    }

    #[test]
    fn lock_without_memory_disables_and_disable_forgets_memory() {
        let list = CallbackList::<(), ()>::default();
        list.lock();
        assert!(list.disabled());
        assert!(!list.fired());
        let list = CallbackList::<(), ()>::new(CallbackOptions::parse("memory"));
        list.fire(()).disable();
        let fired = Rc::new(Cell::new(false));
        let late = {
            let fired = fired.clone();
            Callback::new(move |_, _| {
                fired.set(true);
                true
            })
        };
        list.add(late).fire(());
        assert!(!fired.get());
    }

    #[test]
    fn memory_is_latest_context_and_arguments() {
        let list = CallbackList::new(CallbackOptions::parse("memory"));
        list.fire_with(1, vec![1]).fire_with(2, vec![2, 3]);
        let seen = Rc::new(RefCell::new(None));
        let callback = {
            let seen = seen.clone();
            Callback::new(move |context: &i32, args: &Vec<i32>| {
                *seen.borrow_mut() = Some((*context, args.clone()));
                true
            })
        };
        list.add(callback);
        assert_eq!(*seen.borrow(), Some((2, vec![2, 3])));
    }

    // Expected strings are the vendored test/unit/callbacks.js option matrix.
    // This checks interactions between flags, rather than repeating the state
    // machine in the test's expected-value calculations.
    #[test]
    fn vendored_upstream_option_matrix() {
        let cases = [
            ("", "XABC X XABCABCC X XBB X XABA X XX"),
            ("once", "XABC X X X X X XABA X XX"),
            ("memory", "XABC XABC XABCABCCC XA XBB XB XABA XC XX"),
            ("unique", "XABC X XABCA X XBB X XAB X X"),
            ("stopOnFalse", "XABC X XABCABCC X XBB X XA X XX"),
            ("once memory", "XABC XABC X XA X XA XABA XC XX"),
            ("once unique", "XABC X X X X X XAB X X"),
            ("once stopOnFalse", "XABC X X X X X XA X XX"),
            ("memory unique", "XABC XA XABCA XA XBB XB XAB XC X"),
            (
                "memory stopOnFalse",
                "XABC XABC XABCABCCC XA XBB XB XA X XX",
            ),
            ("unique stopOnFalse", "XABC X XABCA X XBB X XA X X"),
        ];

        fn record(log: &Rc<RefCell<String>>, text: &'static str) -> Callback<(), String> {
            let log = log.clone();
            Callback::new(move |_, _| {
                log.borrow_mut().push_str(text);
                true
            })
        }

        fn record_args(log: &Rc<RefCell<String>>) -> Callback<(), String> {
            let log = log.clone();
            Callback::new(move |_, args: &String| {
                log.borrow_mut().push_str(args);
                true
            })
        }

        fn append_c(
            list: &CallbackList<(), String>,
            log: &Rc<RefCell<String>>,
            c: &Callback<(), String>,
        ) -> Callback<(), String> {
            let list = list.clone();
            let log = log.clone();
            let c = c.clone();
            Callback::new(move |_, _| {
                list.add(c.clone());
                log.borrow_mut().push('A');
                true
            })
        }

        for (flags, expected) in cases {
            let log = Rc::new(RefCell::new(String::from("X")));
            let a = record(&log, "A");
            let b = record(&log, "B");
            let c = record(&log, "C");
            let mut results = Vec::new();
            let save_reset = |results: &mut Vec<String>| {
                results.push(log.borrow().clone());
                *log.borrow_mut() = String::from("X");
            };

            let list = CallbackList::new(CallbackOptions::parse(flags));
            list.add_many([append_c(&list, &log, &c), b.clone()])
                .fire(String::new());
            save_reset(&mut results);
            list.add_many([append_c(&list, &log, &c), b.clone()]);
            save_reset(&mut results);
            list.fire(String::new());
            save_reset(&mut results);

            let list = CallbackList::new(CallbackOptions::parse(flags));
            list.add(record_args(&log)).fire(String::from("A"));
            *log.borrow_mut() = String::from("X");
            list.add(record_args(&log));
            save_reset(&mut results);
            list.fire(String::from("B"));
            save_reset(&mut results);
            list.add(record_args(&log));
            save_reset(&mut results);

            let list = CallbackList::new(CallbackOptions::parse(flags));
            list.add_many([a.clone(), Callback::new(|_, _| false), b.clone()]);
            list.add(a).fire(String::new());
            save_reset(&mut results);
            list.add(c);
            save_reset(&mut results);

            let list = CallbackList::new(CallbackOptions::parse(flags));
            log.borrow_mut().clear();
            let x = record(&log, "X");
            list.add_many([x.clone(), x]).fire(String::new());
            results.push(log.borrow().clone());
            assert_eq!(results.join(" "), expected, "flags: {flags}");
        }
    }
}
