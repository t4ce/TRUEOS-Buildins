//! Native state algorithms from jQuery 4.0.0 `src/deferred.js` (MIT).
//!
//! `done`, `fail`, `progress`, and legacy `pipe` run synchronously; `then`
//! continuations run only when the caller drains a [`TaskQueue`]. No timer,
//! executor, or browser is installed by this module. Values and explicit callback
//! contexts are typed; use a tuple or collection for JavaScript's multiple
//! arguments. `Result` is the native equivalent of a handler returning a value or
//! throwing a rejection. Rust panics are not caught. Only explicit [`Promise`]
//! handles are adopted: JavaScript property lookup, foreign thenables, and the
//! browser console/stack hooks in `deferred/exceptionHook.js` belong to a host.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::rc::{Rc, Weak};
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

use crate::callbacks::{Callback, CallbackList, CallbackOptions};

/// Caller-owned FIFO for the asynchronous boundary used by `then`.
///
/// Draining is bounded by task count, including tasks enqueued by other tasks.
/// A task itself remains a native function; this does not preempt a long task.
#[derive(Clone, Default)]
pub struct TaskQueue {
    tasks: Rc<RefCell<VecDeque<Box<dyn FnOnce()>>>>,
}

#[derive(Clone)]
struct WeakTaskQueue(Weak<RefCell<VecDeque<Box<dyn FnOnce()>>>>);

impl WeakTaskQueue {
    fn upgrade(&self) -> Option<TaskQueue> {
        self.0.upgrade().map(|tasks| TaskQueue { tasks })
    }
}

impl TaskQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&self, task: impl FnOnce() + 'static) {
        self.tasks.borrow_mut().push_back(Box::new(task));
    }

    pub fn pending(&self) -> usize {
        self.tasks.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending() == 0
    }

    fn downgrade(&self) -> WeakTaskQueue {
        WeakTaskQueue(Rc::downgrade(&self.tasks))
    }

    /// Execute at most `limit` tasks, releasing the queue borrow before each.
    pub fn drain(&self, limit: usize) -> usize {
        let mut executed = 0;
        while executed < limit {
            let task = self.tasks.borrow_mut().pop_front();
            let Some(task) = task else { break };
            task();
            executed += 1;
        }
        executed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Pending,
    Resolved,
    Rejected,
}

/// A native counterpart of the TypeError required for self-resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdoptionError {
    SelfResolution,
    Cycle,
}

impl core::fmt::Display for AdoptionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::SelfResolution => "Thenable self-resolution",
            Self::Cycle => "Thenable resolution cycle",
        })
    }
}

impl core::error::Error for AdoptionError {}

struct Inner<T: Clone, E: Clone, P: Clone, C: Clone> {
    state: Cell<State>,
    done: CallbackList<Option<C>, T>,
    fail: CallbackList<Option<C>, E>,
    progress: CallbackList<Option<C>, P>,
    then_done: CallbackList<Option<C>, T>,
    then_fail: CallbackList<Option<C>, E>,
    then_progress: CallbackList<Option<C>, P>,
    // Weak: the source's subscriber keeps the destination alive, never vice versa.
    following: RefCell<Vec<Weak<Inner<T, E, P, C>>>>,
}

/// Writable producer; clones share state and callback identity.
pub struct Deferred<T: Clone, E: Clone = T, P: Clone = T, C: Clone = ()> {
    inner: Rc<Inner<T, E, P, C>>,
}

/// Read-only consumer aspect of a [`Deferred`].
pub struct Promise<T: Clone, E: Clone = T, P: Clone = T, C: Clone = ()> {
    inner: Rc<Inner<T, E, P, C>>,
}

impl<T: Clone, E: Clone, P: Clone, C: Clone> Clone for Deferred<T, E, P, C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: Clone, E: Clone, P: Clone, C: Clone> Clone for Promise<T, E, P, C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Explicit value, rejection, or returned promise from a native `then` handler.
pub enum Outcome<T: Clone, E: Clone = T, P: Clone = T, C: Clone = ()> {
    Resolve(T),
    Reject(E),
    Promise(Promise<T, E, P, C>),
}

/// Legacy `pipe` keeps the current action for ordinary values, but adopts a
/// returned promise's progress, fulfillment, and rejection actions.
pub enum PipeOutcome<A, T: Clone, E: Clone, P: Clone, C: Clone = ()> {
    Value(A),
    Promise(Promise<T, E, P, C>),
}

fn options(once: bool) -> CallbackOptions {
    CallbackOptions {
        once,
        memory: true,
        ..CallbackOptions::default()
    }
}

impl<T: Clone + 'static, E: Clone + 'static, P: Clone + 'static, C: Clone + 'static> Default
    for Deferred<T, E, P, C>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + 'static, E: Clone + 'static, P: Clone + 'static, C: Clone + 'static>
    Deferred<T, E, P, C>
{
    pub fn new() -> Self {
        let deferred = Self {
            inner: Rc::new(Inner {
                state: Cell::new(State::Pending),
                done: CallbackList::new(options(true)),
                fail: CallbackList::new(options(true)),
                progress: CallbackList::new(options(false)),
                then_done: CallbackList::new(options(true)),
                then_fail: CallbackList::new(options(true)),
                then_progress: CallbackList::new(options(false)),
                following: RefCell::new(Vec::new()),
            }),
        };
        // These are the first callbacks of their ordinary lists, exactly like
        // upstream's tuple[3].fire entries. Keeping them inside those lists is
        // significant: a reentrant notify must enqueue the entire next firing,
        // rather than enqueueing its .then handlers ahead of other listeners.
        let handlers = deferred.inner.then_done.clone();
        deferred
            .inner
            .done
            .add(Callback::new(move |context: &Option<C>, value: &T| {
                handlers.fire_with(context.clone(), value.clone());
                true
            }));
        let handlers = deferred.inner.then_fail.clone();
        deferred
            .inner
            .fail
            .add(Callback::new(move |context: &Option<C>, reason: &E| {
                handlers.fire_with(context.clone(), reason.clone());
                true
            }));
        let handlers = deferred.inner.then_progress.clone();
        deferred
            .inner
            .progress
            .add(Callback::new(move |context: &Option<C>, value: &P| {
                handlers.fire_with(context.clone(), value.clone());
                true
            }));
        deferred
    }

    /// jQuery's optional initializer, called synchronously with the producer.
    pub fn with_initializer(initialize: impl FnOnce(&Self)) -> Self {
        let deferred = Self::new();
        initialize(&deferred);
        deferred
    }

    pub fn promise(&self) -> Promise<T, E, P, C> {
        Promise {
            inner: self.inner.clone(),
        }
    }

    pub fn state(&self) -> State {
        self.inner.state.get()
    }

    pub fn resolve(&self, value: T) -> &Self {
        self.resolve_context(None, value);
        self
    }

    pub fn resolve_with(&self, context: C, value: T) -> &Self {
        self.resolve_context(Some(context), value);
        self
    }

    pub fn reject(&self, reason: E) -> &Self {
        self.reject_context(None, reason);
        self
    }

    pub fn reject_with(&self, context: C, reason: E) -> &Self {
        self.reject_context(Some(context), reason);
        self
    }

    pub fn notify(&self, value: P) -> &Self {
        self.notify_context(None, value);
        self
    }

    pub fn notify_with(&self, context: C, value: P) -> &Self {
        self.notify_context(Some(context), value);
        self
    }

    fn resolve_context(&self, context: Option<C>, value: T) {
        if self.state() != State::Pending {
            return;
        }
        self.inner.state.set(State::Resolved);
        self.inner.following.borrow_mut().clear();
        self.inner.fail.disable();
        self.inner.then_fail.disable();
        self.inner.progress.lock();
        self.inner.then_progress.lock();
        self.inner.done.fire_with(context, value);
    }

    fn reject_context(&self, context: Option<C>, reason: E) {
        if self.state() != State::Pending {
            return;
        }
        self.inner.state.set(State::Rejected);
        self.inner.following.borrow_mut().clear();
        self.inner.done.disable();
        self.inner.then_done.disable();
        self.inner.progress.lock();
        self.inner.then_progress.lock();
        self.inner.fail.fire_with(context, reason);
    }

    fn notify_context(&self, context: Option<C>, value: P) {
        if self.state() != State::Pending {
            return;
        }
        self.inner.progress.fire_with(context, value);
    }

    pub fn done(&self, callback: impl Fn(&T) + 'static) -> &Self {
        self.promise().done(callback);
        self
    }

    pub fn done_with(&self, callback: impl Fn(Option<&C>, &T) + 'static) -> &Self {
        self.promise().done_with(callback);
        self
    }

    pub fn fail(&self, callback: impl Fn(&E) + 'static) -> &Self {
        self.promise().fail(callback);
        self
    }

    pub fn fail_with(&self, callback: impl Fn(Option<&C>, &E) + 'static) -> &Self {
        self.promise().fail_with(callback);
        self
    }

    pub fn progress(&self, callback: impl Fn(&P) + 'static) -> &Self {
        self.promise().progress(callback);
        self
    }

    pub fn progress_with(&self, callback: impl Fn(Option<&C>, &P) + 'static) -> &Self {
        self.promise().progress_with(callback);
        self
    }

    pub fn always(&self, callback: impl Fn(Result<&T, &E>) + 'static) -> &Self {
        self.promise().always(callback);
        self
    }

    pub fn always_with(&self, callback: impl Fn(Option<&C>, Result<&T, &E>) + 'static) -> &Self {
        self.promise().always_with(callback);
        self
    }
}

impl<T: Clone + 'static, E: Clone + 'static, P: Clone + 'static, C: Clone + 'static>
    Promise<T, E, P, C>
{
    pub fn state(&self) -> State {
        self.inner.state.get()
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn done(&self, callback: impl Fn(&T) + 'static) -> &Self {
        self.done_with(move |_, value| callback(value))
    }

    pub fn done_with(&self, callback: impl Fn(Option<&C>, &T) + 'static) -> &Self {
        self.inner
            .done
            .add(Callback::new(move |context: &Option<C>, value: &T| {
                callback(context.as_ref(), value);
                true
            }));
        self
    }

    pub fn fail(&self, callback: impl Fn(&E) + 'static) -> &Self {
        self.fail_with(move |_, reason| callback(reason))
    }

    pub fn fail_with(&self, callback: impl Fn(Option<&C>, &E) + 'static) -> &Self {
        self.inner
            .fail
            .add(Callback::new(move |context: &Option<C>, reason: &E| {
                callback(context.as_ref(), reason);
                true
            }));
        self
    }

    pub fn progress(&self, callback: impl Fn(&P) + 'static) -> &Self {
        self.progress_with(move |_, value| callback(value))
    }

    pub fn progress_with(&self, callback: impl Fn(Option<&C>, &P) + 'static) -> &Self {
        self.inner
            .progress
            .add(Callback::new(move |context: &Option<C>, value: &P| {
                callback(context.as_ref(), value);
                true
            }));
        self
    }

    pub fn always(&self, callback: impl Fn(Result<&T, &E>) + 'static) -> &Self {
        self.always_with(move |_, result| callback(result))
    }

    pub fn always_with(&self, callback: impl Fn(Option<&C>, Result<&T, &E>) + 'static) -> &Self {
        let callback = Rc::new(callback);
        let on_done = callback.clone();
        self.done_with(move |context, value| on_done(context, Ok(value)));
        self.fail_with(move |context, reason| callback(context, Err(reason)));
        self
    }

    /// Asynchronous fulfillment filter; rejection and progress pass through.
    /// A returned `Err` models a handler throwing; it rejects the child.
    pub fn then<U, F>(&self, tasks: &TaskQueue, fulfilled: F) -> Promise<U, E, P, C>
    where
        U: Clone + 'static,
        F: Fn(&T) -> Result<U, E> + 'static,
    {
        let child = Deferred::new();
        self.forward_progress(tasks, &child);
        self.forward_failure(tasks, &child);
        let target = child.clone();
        self.on_then_done(tasks, move |_, value| match fulfilled(&value) {
            Ok(value) => {
                target.resolve(value);
            }
            Err(reason) => {
                target.reject(reason);
            }
        });
        child.promise()
    }

    /// Fulfillment and rejection filters. Either may recover with `Ok`, or
    /// reject with `Err`. Custom filters clear the original context, as `.then`
    /// does; use `then_identity` when forwarding it unchanged.
    pub fn then_with<U, F, G>(
        &self,
        tasks: &TaskQueue,
        fulfilled: F,
        rejected: G,
    ) -> Promise<U, E, P, C>
    where
        U: Clone + 'static,
        F: Fn(Option<&C>, &T) -> Result<U, E> + 'static,
        G: Fn(Option<&C>, &E) -> Result<U, E> + 'static,
    {
        let child = Deferred::new();
        self.forward_progress(tasks, &child);
        let target = child.clone();
        self.on_then_done(tasks, move |context, value| {
            match fulfilled(context.as_ref(), &value) {
                Ok(value) => {
                    target.resolve(value);
                }
                Err(reason) => {
                    target.reject(reason);
                }
            }
        });
        let target = child.clone();
        self.on_then_fail(tasks, move |context, reason| {
            match rejected(context.as_ref(), &reason) {
                Ok(value) => {
                    target.resolve(value);
                }
                Err(reason) => {
                    target.reject(reason);
                }
            }
        });
        child.promise()
    }

    /// The absent-handler identity/thrower behavior of JavaScript `.then()`.
    pub fn then_identity(&self, tasks: &TaskQueue) -> Self {
        let child = Deferred::new();
        self.forward_progress(tasks, &child);
        self.forward_failure(tasks, &child);
        let target = child.clone();
        self.on_then_done(tasks, move |context, value| {
            target.resolve_context(context, value)
        });
        child.promise()
    }

    pub fn catch<F>(&self, tasks: &TaskQueue, rejected: F) -> Self
    where
        F: Fn(&E) -> Result<T, E> + 'static,
    {
        let child = Deferred::new();
        self.forward_progress(tasks, &child);
        let target = child.clone();
        self.on_then_done(tasks, move |context, value| {
            target.resolve_context(context, value)
        });
        let target = child.clone();
        self.on_then_fail(tasks, move |_, reason| match rejected(&reason) {
            Ok(value) => {
                target.resolve(value);
            }
            Err(reason) => {
                target.reject(reason);
            }
        });
        child.promise()
    }

    /// Map progress on the same asynchronous boundary as `.then` handlers.
    /// Like upstream's progress processor, a panic is not turned into rejection.
    pub fn then_progress<Q, F>(&self, tasks: &TaskQueue, progress: F) -> Promise<T, E, Q, C>
    where
        Q: Clone + 'static,
        F: Fn(&P) -> Q + 'static,
    {
        let child = Deferred::new();
        let target = child.clone();
        self.on_then_progress(tasks, move |_, value| {
            target.notify(progress(&value));
        });
        let target = child.clone();
        self.on_then_done(tasks, move |context, value| {
            target.resolve_context(context, value)
        });
        let target = child.clone();
        self.on_then_fail(tasks, move |context, reason| {
            target.reject_context(context, reason)
        });
        child.promise()
    }

    /// A promise returned by a `.then` progress filter contributes only its
    /// fulfillment as a notification; its own progress is ignored. Rejection is
    /// sent to the caller's uncaught-error callback, rather than rejecting the
    /// child, matching upstream's special progress processor. No console or
    /// process-global error handler is installed.
    pub fn then_progress_adopt<Q, R, F, H>(
        &self,
        tasks: &TaskQueue,
        progress: F,
        on_unhandled_rejection: H,
    ) -> Promise<T, E, Q, C>
    where
        Q: Clone + 'static,
        R: Clone + 'static,
        E: From<AdoptionError>,
        F: Fn(&P) -> Promise<Q, E, R, C> + 'static,
        H: Fn(Option<&C>, &E) + 'static,
    {
        let child = Deferred::new();
        let target = child.clone();
        let queue = tasks.downgrade();
        let on_unhandled_rejection = Rc::new(on_unhandled_rejection);
        self.on_then_progress(tasks, move |_, value| {
            let source = progress(&value);
            if Rc::as_ptr(&source.inner).cast::<()>() == Rc::as_ptr(&target.inner).cast::<()>() {
                on_unhandled_rejection(None, &E::from(AdoptionError::SelfResolution));
                return;
            }
            let Some(queue) = queue.upgrade() else { return };
            let notify = target.clone();
            source.on_then_done(&queue, move |context, value| {
                notify.notify_context(context, value)
            });
            let on_error = on_unhandled_rejection.clone();
            source.on_then_fail(&queue, move |context, reason| {
                on_error(context.as_ref(), &reason)
            });
        });
        let target = child.clone();
        self.on_then_done(tasks, move |context, value| {
            target.resolve_context(context, value)
        });
        let target = child.clone();
        self.on_then_fail(tasks, move |context, reason| {
            target.reject_context(context, reason)
        });
        child.promise()
    }

    /// Native returned-promise adoption, including self-resolution checks.
    /// Errors are explicit so the host can choose its own TypeError value.
    pub fn then_outcome<U, F, G>(
        &self,
        tasks: &TaskQueue,
        fulfilled: F,
        rejected: G,
    ) -> Promise<U, E, P, C>
    where
        U: Clone + 'static,
        E: From<AdoptionError>,
        F: Fn(Option<&C>, &T) -> Outcome<U, E, P, C> + 'static,
        G: Fn(Option<&C>, &E) -> Outcome<U, E, P, C> + 'static,
    {
        let child = Deferred::new();
        self.forward_progress(tasks, &child);
        let target = child.clone();
        let queue = tasks.downgrade();
        self.on_then_done(tasks, move |context, value| {
            if let Some(queue) = queue.upgrade() {
                apply_outcome(&target, &queue, fulfilled(context.as_ref(), &value));
            }
        });
        let target = child.clone();
        let queue = tasks.downgrade();
        self.on_then_fail(tasks, move |context, reason| {
            if let Some(queue) = queue.upgrade() {
                apply_outcome(&target, &queue, rejected(context.as_ref(), &reason));
            }
        });
        child.promise()
    }

    pub fn then_adopt<U, F>(&self, tasks: &TaskQueue, fulfilled: F) -> Promise<U, E, P, C>
    where
        U: Clone + 'static,
        E: From<AdoptionError>,
        F: Fn(&T) -> Promise<U, E, P, C> + 'static,
    {
        // An absent rejection handler must preserve its original context.
        let child = Deferred::new();
        self.forward_progress(tasks, &child);
        self.forward_failure(tasks, &child);
        let target = child.clone();
        let queue = tasks.downgrade();
        self.on_then_done(tasks, move |_, value| {
            if let Some(queue) = queue.upgrade() {
                apply_outcome(&target, &queue, Outcome::Promise(fulfilled(&value)));
            }
        });
        child.promise()
    }

    /// Legacy synchronous filtering: a fail filter still rejects; a progress
    /// filter still notifies. This deliberately differs from `then` recovery.
    pub fn pipe<U, V, Q, F, G, H>(
        &self,
        fulfilled: F,
        rejected: G,
        progress: H,
    ) -> Promise<U, V, Q, C>
    where
        U: Clone + 'static,
        V: Clone + 'static,
        Q: Clone + 'static,
        F: Fn(&T) -> U + 'static,
        G: Fn(&E) -> V + 'static,
        H: Fn(&P) -> Q + 'static,
    {
        let child = Deferred::new();
        let target = child.clone();
        self.progress_with(move |context, value| {
            target.notify_context(context.cloned(), progress(value))
        });
        let target = child.clone();
        self.done_with(move |context, value| {
            target.resolve_context(context.cloned(), fulfilled(value))
        });
        let target = child.clone();
        self.fail_with(move |context, reason| {
            target.reject_context(context.cloned(), rejected(reason))
        });
        child.promise()
    }

    pub fn pipe_identity(&self) -> Self {
        self.pipe(Clone::clone, Clone::clone, Clone::clone)
    }

    /// Full native legacy pipe return handling. A promise returned even from a
    /// progress filter is adopted for all three actions (unlike `.then`).
    pub fn pipe_outcome<U, V, Q, F, G, H>(
        &self,
        fulfilled: F,
        rejected: G,
        progress: H,
    ) -> Promise<U, V, Q, C>
    where
        U: Clone + 'static,
        V: Clone + From<AdoptionError> + 'static,
        Q: Clone + 'static,
        F: Fn(Option<&C>, &T) -> PipeOutcome<U, U, V, Q, C> + 'static,
        G: Fn(Option<&C>, &E) -> PipeOutcome<V, U, V, Q, C> + 'static,
        H: Fn(Option<&C>, &P) -> PipeOutcome<Q, U, V, Q, C> + 'static,
    {
        let child = Deferred::new();
        let target = child.clone();
        self.progress_with(move |context, value| match progress(context, value) {
            PipeOutcome::Value(value) => target.notify_context(context.cloned(), value),
            PipeOutcome::Promise(source) => follow_or_reject(&target, &source, None),
        });
        let target = child.clone();
        self.done_with(move |context, value| match fulfilled(context, value) {
            PipeOutcome::Value(value) => target.resolve_context(context.cloned(), value),
            PipeOutcome::Promise(source) => follow_or_reject(&target, &source, None),
        });
        let target = child.clone();
        self.fail_with(move |context, reason| match rejected(context, reason) {
            PipeOutcome::Value(reason) => target.reject_context(context.cloned(), reason),
            PipeOutcome::Promise(source) => follow_or_reject(&target, &source, None),
        });
        child.promise()
    }

    fn on_then_done(&self, tasks: &TaskQueue, callback: impl Fn(Option<C>, T) + 'static) {
        enqueue_listener(&self.inner.then_done, tasks, callback);
    }

    fn on_then_fail(&self, tasks: &TaskQueue, callback: impl Fn(Option<C>, E) + 'static) {
        enqueue_listener(&self.inner.then_fail, tasks, callback);
    }

    fn on_then_progress(&self, tasks: &TaskQueue, callback: impl Fn(Option<C>, P) + 'static) {
        enqueue_listener(&self.inner.then_progress, tasks, callback);
    }

    fn forward_progress<U: Clone + 'static>(
        &self,
        tasks: &TaskQueue,
        target: &Deferred<U, E, P, C>,
    ) {
        let target = target.clone();
        self.on_then_progress(tasks, move |context, value| {
            target.notify_context(context, value)
        });
    }

    fn forward_failure<U: Clone + 'static>(
        &self,
        tasks: &TaskQueue,
        target: &Deferred<U, E, P, C>,
    ) {
        let target = target.clone();
        self.on_then_fail(tasks, move |context, reason| {
            target.reject_context(context, reason)
        });
    }
}

fn enqueue_listener<C: Clone + 'static, A: Clone + 'static>(
    list: &CallbackList<Option<C>, A>,
    tasks: &TaskQueue,
    callback: impl Fn(Option<C>, A) + 'static,
) {
    let callback = Rc::new(callback);
    let queue = tasks.downgrade();
    list.add(Callback::new(move |context: &Option<C>, argument: &A| {
        let context = context.clone();
        let argument = argument.clone();
        let callback = callback.clone();
        if let Some(queue) = queue.upgrade() {
            queue.enqueue(move || callback(context, argument));
        }
        true
    }));
}

fn apply_outcome<T, E, P, C>(
    target: &Deferred<T, E, P, C>,
    tasks: &TaskQueue,
    outcome: Outcome<T, E, P, C>,
) where
    T: Clone + 'static,
    E: Clone + From<AdoptionError> + 'static,
    P: Clone + 'static,
    C: Clone + 'static,
{
    match outcome {
        Outcome::Resolve(value) => {
            target.resolve(value);
        }
        Outcome::Reject(reason) => {
            target.reject(reason);
        }
        Outcome::Promise(source) => follow_or_reject(target, &source, Some(tasks)),
    }
}

fn follow_or_reject<T, E, P, C>(
    target: &Deferred<T, E, P, C>,
    source: &Promise<T, E, P, C>,
    tasks: Option<&TaskQueue>,
) where
    T: Clone + 'static,
    E: Clone + From<AdoptionError> + 'static,
    P: Clone + 'static,
    C: Clone + 'static,
{
    if target.state() != State::Pending {
        return;
    }
    if Rc::ptr_eq(&target.inner, &source.inner) {
        target.reject(E::from(AdoptionError::SelfResolution));
        return;
    }
    let mut pending = alloc::vec![source.inner.clone()];
    let mut visited = Vec::new();
    while let Some(inner) = pending.pop() {
        if Rc::ptr_eq(&target.inner, &inner) {
            target.reject(E::from(AdoptionError::Cycle));
            return;
        }
        let identity = Rc::as_ptr(&inner);
        if visited.contains(&identity) {
            continue;
        }
        visited.push(identity);
        pending.extend(inner.following.borrow().iter().filter_map(Weak::upgrade));
    }
    target
        .inner
        .following
        .borrow_mut()
        .push(Rc::downgrade(&source.inner));
    if let Some(tasks) = tasks {
        let child = target.clone();
        source.on_then_progress(tasks, move |context, value| {
            child.notify_context(context, value)
        });
        let child = target.clone();
        source.on_then_done(tasks, move |context, value| {
            child.resolve_context(context, value)
        });
        let child = target.clone();
        source.on_then_fail(tasks, move |context, reason| {
            child.reject_context(context, reason)
        });
    } else {
        let child = target.clone();
        source.progress_with(move |context, value| {
            child.notify_context(context.cloned(), value.clone())
        });
        let child = target.clone();
        source.done_with(move |context, value| {
            child.resolve_context(context.cloned(), value.clone())
        });
        let child = target.clone();
        source.fail_with(move |context, reason| {
            child.reject_context(context.cloned(), reason.clone())
        });
    }
}

/// Inputs to [`when`]. Ordinary values do not undergo implicit thenable lookup.
pub enum When<T: Clone, E: Clone = T, P: Clone = T, C: Clone = ()> {
    Value(T),
    Promise(Promise<T, E, P, C>),
}

/// Typed representation of `when`'s ordered values and resolution contexts.
#[derive(Clone, Debug, PartialEq)]
pub struct WhenValues<T, C = ()> {
    pub values: Vec<T>,
    pub contexts: Vec<Option<C>>,
}

/// Aggregate values/promises in input order, rejecting on the first failure.
///
/// Like upstream, empty inputs and already-settled inputs can complete
/// synchronously. A single pending promise crosses the `then` boundary; multiple
/// inputs subscribe synchronously. Aggregate success contexts are explicit in
/// [`WhenValues`]; a rejection retains the failing subordinate's context.
pub fn when<T, E, P, C>(
    tasks: &TaskQueue,
    inputs: impl IntoIterator<Item = When<T, E, P, C>>,
) -> Promise<WhenValues<T, C>, E, (), C>
where
    T: Clone + 'static,
    E: Clone + 'static,
    P: Clone + 'static,
    C: Clone + 'static,
{
    let inputs: Vec<_> = inputs.into_iter().collect();
    let count = inputs.len();
    let primary = Deferred::new();
    if count == 0 {
        primary.resolve(WhenValues {
            values: Vec::new(),
            contexts: Vec::new(),
        });
        return primary.promise();
    }
    let single_pending =
        count == 1 && matches!(&inputs[0], When::Promise(p) if p.state() == State::Pending);
    let remaining = Rc::new(Cell::new(count));
    let values: Rc<RefCell<Vec<Option<T>>>> =
        Rc::new(RefCell::new((0..count).map(|_| None).collect()));
    let contexts: Rc<RefCell<Vec<Option<C>>>> =
        Rc::new(RefCell::new((0..count).map(|_| None).collect()));
    // Upstream walks its input array backwards when adopting multiple values.
    for (index, input) in inputs.into_iter().enumerate().rev() {
        let target = primary.clone();
        let remaining = remaining.clone();
        let values = values.clone();
        let contexts = contexts.clone();
        let update = move |context: Option<C>, value: T| {
            values.borrow_mut()[index] = Some(value);
            contexts.borrow_mut()[index] = context;
            let left = remaining.get() - 1;
            remaining.set(left);
            if left == 0 {
                let values = values
                    .borrow()
                    .iter()
                    .map(|v| v.clone().expect("completed when input"))
                    .collect();
                let contexts = contexts.borrow().clone();
                target.resolve(WhenValues { values, contexts });
            }
        };
        match input {
            When::Value(value) => update(None, value),
            When::Promise(source) => {
                let target = primary.clone();
                if single_pending {
                    source.on_then_done(tasks, update);
                    source.on_then_fail(tasks, move |context, reason| {
                        target.reject_context(context, reason)
                    });
                } else {
                    source.done_with(move |context, value| update(context.cloned(), value.clone()));
                    source.fail_with(move |context, reason| {
                        target.reject_context(context.cloned(), reason.clone())
                    });
                }
            }
        }
    }
    primary.promise()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;

    type D = Deferred<i32, i32, i32, &'static str>;

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum Failure {
        Rejected(&'static str),
        Adoption(AdoptionError),
    }

    impl From<AdoptionError> for Failure {
        fn from(error: AdoptionError) -> Self {
            Self::Adoption(error)
        }
    }

    type Adopting = Deferred<i32, Failure, i32, &'static str>;

    #[test]
    fn terminal_state_is_visible_before_once_memory_callbacks() {
        let deferred = D::with_initializer(|d| {
            d.resolve_with("page", 7);
        });
        let observed = Rc::new(RefCell::new(Vec::new()));
        let output = observed.clone();
        let inspect = deferred.clone();
        deferred.done_with(move |context, value| {
            output
                .borrow_mut()
                .push((inspect.state(), context.copied(), *value));
        });
        deferred.reject(9).resolve(10).notify(11);
        assert_eq!(deferred.state(), State::Resolved);
        assert_eq!(*observed.borrow(), vec![(State::Resolved, Some("page"), 7)]);
        deferred.fail(|_| panic!("opposite callback list must be disabled"));
        assert!(deferred.promise().ptr_eq(&deferred.promise()));
    }

    #[test]
    fn rejection_has_once_memory_and_always_runs_for_either_action() {
        let rejected = D::new();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let output = calls.clone();
        rejected.always(move |result| output.borrow_mut().push(result.map(|v| *v).map_err(|e| *e)));
        rejected.reject(4).reject(5).resolve(6);
        let output = calls.clone();
        rejected.fail(move |value| output.borrow_mut().push(Err(*value)));
        let resolved = D::new();
        let output = calls.clone();
        resolved.always(move |result| output.borrow_mut().push(result.map(|v| *v).map_err(|e| *e)));
        resolved.resolve(8);
        assert_eq!(*calls.borrow(), vec![Err(4), Err(4), Ok(8)]);
        rejected.done(|_| panic!("resolved callbacks cannot run after rejection"));
    }

    #[test]
    fn terminal_locks_progress_but_keeps_the_last_progress_memory() {
        let deferred = D::new();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let output = calls.clone();
        deferred.progress(move |value| output.borrow_mut().push(*value));
        deferred.notify(1).notify(2).resolve(3).notify(4);
        let output = calls.clone();
        deferred.progress(move |value| output.borrow_mut().push(*value * 10));
        assert_eq!(*calls.borrow(), vec![1, 2, 20]);

        let never_notified = D::new();
        never_notified.reject(5);
        never_notified.progress(|_| panic!("no invented progress memory"));
    }

    #[test]
    fn callbacks_added_during_done_are_reentrant_and_run_in_order() {
        let deferred = D::new();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let output = calls.clone();
        let producer = deferred.clone();
        deferred.done(move |_| {
            output.borrow_mut().push(1);
            let output = output.clone();
            producer.done(move |_| output.borrow_mut().push(3));
        });
        let output = calls.clone();
        deferred.done(move |_| output.borrow_mut().push(2));
        deferred.resolve(10);
        assert_eq!(*calls.borrow(), vec![1, 2, 3]);
    }

    #[test]
    fn bounded_task_queue_preserves_fifo_including_followup_tasks() {
        let tasks = TaskQueue::new();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let output = calls.clone();
        let queue = tasks.clone();
        tasks.enqueue(move || {
            output.borrow_mut().push(1);
            queue.enqueue(move || output.borrow_mut().push(3));
        });
        let output = calls.clone();
        tasks.enqueue(move || output.borrow_mut().push(2));
        assert_eq!(tasks.drain(0), 0);
        assert_eq!(tasks.drain(2), 2);
        assert_eq!(*calls.borrow(), vec![1, 2]);
        assert_eq!(tasks.pending(), 1);
        assert_eq!(tasks.drain(10), 1);
        assert_eq!(*calls.borrow(), vec![1, 2, 3]);
        assert!(tasks.is_empty());
    }

    #[test]
    fn async_continuations_are_enqueued_before_user_done_callbacks() {
        let tasks = TaskQueue::new();
        let deferred = D::new();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let output = calls.clone();
        let queue = tasks.clone();
        deferred.done(move |_| {
            output.borrow_mut().push(0);
            let output = output.clone();
            queue.enqueue(move || output.borrow_mut().push(2));
        });
        let output = calls.clone();
        let child = deferred.promise().then(&tasks, move |value| {
            output.borrow_mut().push(1);
            Ok(value * 2)
        });
        let result = Rc::new(Cell::new(0));
        let output = result.clone();
        child.done(move |value| output.set(*value));
        deferred.resolve(5);
        assert_eq!(*calls.borrow(), vec![0]);
        assert_eq!(child.state(), State::Pending);
        assert_eq!(tasks.drain(1), 1);
        assert_eq!(result.get(), 10);
        assert_eq!(*calls.borrow(), vec![0, 1]);
        tasks.drain(10);
        assert_eq!(*calls.borrow(), vec![0, 1, 2]);
    }

    #[test]
    fn result_filters_reject_and_rejection_filters_can_recover() {
        let tasks = TaskQueue::new();
        let source = D::new();
        let rejected = source.promise().then(&tasks, |_| Err::<i32, i32>(99));
        let recovered = rejected.catch(&tasks, |reason| Ok(reason + 1));
        let calls = Rc::new(RefCell::new(Vec::new()));
        let output = calls.clone();
        recovered.done(move |value| output.borrow_mut().push(*value));
        source.resolve(2);
        assert_eq!(tasks.drain(1), 1);
        assert_eq!(rejected.state(), State::Rejected);
        assert_eq!(recovered.state(), State::Pending);
        assert_eq!(tasks.drain(1), 1);
        assert_eq!(*calls.borrow(), vec![100]);

        let source = D::new();
        let recovered = source.promise().then_with(
            &tasks,
            |_, v| Ok(*v),
            |c, v| {
                assert_eq!(c, Some(&"failure"));
                Ok(v * 3)
            },
        );
        source.reject_with("failure", 4);
        tasks.drain(10);
        assert_eq!(recovered.state(), State::Resolved);
    }

    #[test]
    fn identity_preserves_context_custom_then_clears_it_and_pipe_keeps_it() {
        let tasks = TaskQueue::new();
        let source = D::new();
        let identity = source.promise().then_identity(&tasks);
        let filtered = source.promise().then(&tasks, |v| Ok(v * 2));
        let piped = source.promise().pipe(|v| v * 3, |v| *v, |v| *v);
        let calls = Rc::new(RefCell::new(Vec::new()));
        for (label, promise) in [
            ("identity", identity),
            ("filtered", filtered),
            ("piped", piped),
        ] {
            let output = calls.clone();
            promise.done_with(move |context, value| {
                output.borrow_mut().push((label, context.copied(), *value))
            });
        }
        source.resolve_with("document", 2);
        assert_eq!(*calls.borrow(), vec![("piped", Some("document"), 6)]);
        tasks.drain(10);
        assert_eq!(
            *calls.borrow(),
            vec![
                ("piped", Some("document"), 6),
                ("identity", Some("document"), 2),
                ("filtered", None, 4),
            ]
        );
    }

    #[test]
    fn late_then_replays_saved_progress_before_completion() {
        let tasks = TaskQueue::new();
        let source = D::new();
        source.notify_with("progress", 3).resolve_with("done", 4);
        let child = source.promise().then_identity(&tasks);
        let calls = Rc::new(RefCell::new(Vec::new()));
        let output = calls.clone();
        child.progress_with(move |context, value| {
            output.borrow_mut().push((context.copied(), *value))
        });
        let output = calls.clone();
        child.done_with(move |context, value| output.borrow_mut().push((context.copied(), *value)));
        assert_eq!(tasks.pending(), 2);
        tasks.drain(10);
        assert_eq!(
            *calls.borrow(),
            vec![(Some("progress"), 3), (Some("done"), 4)]
        );
        let output = calls.clone();
        child.progress_with(move |context, value| {
            output.borrow_mut().push((context.copied(), *value))
        });
        assert_eq!(calls.borrow().last(), Some(&(Some("progress"), 3)));
    }

    #[test]
    fn then_progress_maps_queued_notifications_without_resolving() {
        let tasks = TaskQueue::new();
        let source = D::new();
        let child = source.promise().then_progress(&tasks, |v| v * 10);
        let progress = Rc::new(Cell::new(0));
        let output = progress.clone();
        child.progress(move |value| output.set(*value));
        source.notify(2);
        assert_eq!(progress.get(), 0);
        tasks.drain(10);
        assert_eq!(progress.get(), 20);
        assert_eq!(child.state(), State::Pending);
        source.resolve(5);
        tasks.drain(10);
        assert_eq!(child.state(), State::Resolved);
    }

    #[test]
    fn recursive_notify_schedules_the_whole_next_firing_in_order() {
        let tasks = TaskQueue::new();
        let source = D::new();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let child = source.promise().then_progress(&tasks, |value| value * 10);
        let output = calls.clone();
        child.progress(move |value| output.borrow_mut().push(*value));
        let recurse = source.clone();
        let queue = tasks.clone();
        let output = calls.clone();
        source.progress(move |value| {
            if *value == 1 {
                recurse.notify(2);
                let output = output.clone();
                queue.enqueue(move || output.borrow_mut().push(9));
            }
        });
        source.notify(1);
        tasks.drain(10);
        assert_eq!(*calls.borrow(), vec![10, 9, 20]);
        source.resolve(0); // Release the self-capturing progress subscriber.
    }

    #[test]
    fn then_progress_adoption_ignores_returned_progress_and_only_notifies() {
        let tasks = TaskQueue::new();
        let source = Adopting::new();
        let returned = Adopting::new();
        returned.notify(3);
        let handle = returned.promise();
        let child = source.promise().then_progress_adopt(
            &tasks,
            move |_| handle.clone(),
            |_, error| panic!("unexpected progress rejection: {error:?}"),
        );
        let calls = Rc::new(RefCell::new(Vec::new()));
        let output = calls.clone();
        child.progress(move |value| output.borrow_mut().push(*value));
        source.notify(1);
        tasks.drain(10);
        assert!(calls.borrow().is_empty());
        returned.notify(4).resolve(5);
        tasks.drain(10);
        assert_eq!(*calls.borrow(), vec![5]);
        assert_eq!(child.state(), State::Pending);
        source.resolve(9);
        tasks.drain(10);
        assert_eq!(child.state(), State::Resolved);
    }

    #[test]
    fn rejected_progress_return_is_reported_without_rejecting_child() {
        let tasks = TaskQueue::new();
        let source = Adopting::new();
        let returned = Adopting::new();
        returned.reject_with("returned", Failure::Rejected("progress error"));
        let handle = returned.promise();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let output = calls.clone();
        let child = source.promise().then_progress_adopt(
            &tasks,
            move |_| handle.clone(),
            move |context, reason| output.borrow_mut().push((context.copied(), reason.clone())),
        );
        source.notify(1);
        tasks.drain(10);
        assert_eq!(child.state(), State::Pending);
        assert_eq!(
            *calls.borrow(),
            vec![(Some("returned"), Failure::Rejected("progress error"))]
        );
    }

    #[test]
    fn legacy_pipe_fail_filter_keeps_rejection_channel_synchronously() {
        let source = D::new();
        let child = source.promise().pipe(|v| v * 2, |v| v * 3, |v| v * 4);
        let result = Rc::new(Cell::new(0));
        let output = result.clone();
        child.fail(move |reason| output.set(*reason));
        source.reject(7);
        assert_eq!(child.state(), State::Rejected);
        assert_eq!(result.get(), 21);
    }

    #[test]
    fn then_adopts_returned_promise_on_another_async_boundary() {
        let tasks = TaskQueue::new();
        let source = Adopting::new();
        let returned = Adopting::new();
        returned.notify_with("loading", 3).resolve_with("ready", 9);
        let returned_promise = returned.promise();
        let child = source
            .promise()
            .then_adopt(&tasks, move |_| returned_promise.clone());
        let calls = Rc::new(RefCell::new(Vec::new()));
        let output = calls.clone();
        child.progress_with(move |context, value| {
            output.borrow_mut().push((context.copied(), *value))
        });
        let output = calls.clone();
        child.done_with(move |context, value| output.borrow_mut().push((context.copied(), *value)));
        source.resolve(0);
        assert_eq!(tasks.drain(1), 1);
        assert_eq!(child.state(), State::Pending);
        assert_eq!(tasks.pending(), 2);
        tasks.drain(10);
        assert_eq!(
            *calls.borrow(),
            vec![(Some("loading"), 3), (Some("ready"), 9)]
        );
        assert_eq!(child.state(), State::Resolved);
    }

    #[test]
    fn adopted_promise_can_reject_and_cannot_be_overwritten() {
        let tasks = TaskQueue::new();
        let source = Adopting::new();
        let returned = Adopting::new();
        let returned_promise = returned.promise();
        let child = source
            .promise()
            .then_adopt(&tasks, move |_| returned_promise.clone());
        source.resolve(1);
        tasks.drain(10);
        assert_eq!(child.state(), State::Pending);
        returned.reject(Failure::Rejected("denied")).resolve(2);
        assert_eq!(child.state(), State::Pending);
        tasks.drain(10);
        assert_eq!(child.state(), State::Rejected);
        child.fail(|reason| assert_eq!(reason, &Failure::Rejected("denied")));
    }

    #[test]
    fn legacy_pipe_adopts_progress_return_for_all_actions_synchronously() {
        let source = Adopting::new();
        let returned = Adopting::new();
        returned.notify(3).resolve(8);
        let returned_promise = returned.promise();
        let child = source.promise().pipe_outcome(
            |_, value| PipeOutcome::Value(*value),
            |_, error| PipeOutcome::Value(error.clone()),
            move |_, _| PipeOutcome::Promise(returned_promise.clone()),
        );
        let values = Rc::new(RefCell::new(Vec::new()));
        let output = values.clone();
        child.progress(move |v| output.borrow_mut().push(*v));
        let output = values.clone();
        child.done(move |v| output.borrow_mut().push(*v));
        source.notify(1);
        assert_eq!(child.state(), State::Resolved);
        assert_eq!(*values.borrow(), vec![3, 8]);
    }

    #[test]
    fn then_outcome_can_resolve_recover_or_reject_explicitly() {
        let tasks = TaskQueue::new();
        let source = Adopting::new();
        let child = source.promise().then_outcome(
            &tasks,
            |_, value| Outcome::Resolve(value * 2),
            |_, _| Outcome::Resolve(99),
        );
        source.reject(Failure::Rejected("recover"));
        tasks.drain(10);
        child.done(|value| assert_eq!(*value, 99));
        let rejected = child.then_outcome(
            &tasks,
            |_, _| {
                Outcome::<i32, Failure, i32, &'static str>::Reject(Failure::Rejected("explicit"))
            },
            |_, error| Outcome::Reject(error.clone()),
        );
        tasks.drain(10);
        assert_eq!(rejected.state(), State::Rejected);
    }

    #[test]
    fn self_resolution_rejects_instead_of_leaving_a_pending_cycle() {
        let tasks = TaskQueue::new();
        let source = Adopting::new();
        let slot = Rc::new(RefCell::new(
            None::<Promise<i32, Failure, i32, &'static str>>,
        ));
        let handle = slot.clone();
        let child = source
            .promise()
            .then_adopt(&tasks, move |_| handle.borrow().as_ref().unwrap().clone());
        *slot.borrow_mut() = Some(child.clone());
        source.resolve(1);
        tasks.drain(10);
        assert_eq!(child.state(), State::Rejected);
        child.fail(|reason| assert_eq!(reason, &Failure::Adoption(AdoptionError::SelfResolution)));
        slot.borrow_mut().take();
    }

    #[test]
    fn indirect_adoption_cycle_rejects_both_participants() {
        let tasks = TaskQueue::new();
        let source_a = Adopting::new();
        let source_b = Adopting::new();
        let a_slot = Rc::new(RefCell::new(
            None::<Promise<i32, Failure, i32, &'static str>>,
        ));
        let b_slot = Rc::new(RefCell::new(
            None::<Promise<i32, Failure, i32, &'static str>>,
        ));
        let handle = b_slot.clone();
        let child_a = source_a
            .promise()
            .then_adopt(&tasks, move |_| handle.borrow().as_ref().unwrap().clone());
        let handle = a_slot.clone();
        let child_b = source_b
            .promise()
            .then_adopt(&tasks, move |_| handle.borrow().as_ref().unwrap().clone());
        *a_slot.borrow_mut() = Some(child_a.clone());
        *b_slot.borrow_mut() = Some(child_b.clone());
        source_a.resolve(1);
        source_b.resolve(2);
        tasks.drain(20);
        assert_eq!(child_a.state(), State::Rejected);
        assert_eq!(child_b.state(), State::Rejected);
        child_a.fail(|reason| assert_eq!(reason, &Failure::Adoption(AdoptionError::Cycle)));
        a_slot.borrow_mut().take();
        b_slot.borrow_mut().take();
    }

    #[test]
    fn when_collects_in_input_order_and_keeps_resolution_contexts() {
        let tasks = TaskQueue::new();
        let first = D::new();
        let second = D::new();
        let result = when(
            &tasks,
            [
                When::Promise(first.promise()),
                When::Value(2),
                When::Promise(second.promise()),
            ],
        );
        second.notify(99).resolve_with("second", 3);
        assert_eq!(result.state(), State::Pending);
        first.resolve_with("first", 1);
        assert_eq!(result.state(), State::Resolved);
        result.done(|values| {
            assert_eq!(values.values, vec![1, 2, 3]);
            assert_eq!(values.contexts, vec![Some("first"), None, Some("second")]);
        });
        assert!(tasks.is_empty());
    }

    #[test]
    fn when_pending_single_is_async_but_multiple_inputs_are_synchronous() {
        let tasks = TaskQueue::new();
        let single = D::new();
        let result = when(&tasks, [When::Promise(single.promise())]);
        single.resolve(5);
        assert_eq!(result.state(), State::Pending);
        tasks.drain(10);
        assert_eq!(result.state(), State::Resolved);
        let many = D::new();
        let result = when(&tasks, [When::Promise(many.promise()), When::Value(6)]);
        many.resolve(5);
        assert_eq!(result.state(), State::Resolved);
        assert!(tasks.is_empty());
    }

    #[test]
    fn when_empty_values_and_already_settled_sources_are_synchronous() {
        let tasks = TaskQueue::new();
        let empty = when::<i32, i32, i32, &'static str>(&tasks, []);
        assert_eq!(empty.state(), State::Resolved);
        empty.done(|values| assert!(values.values.is_empty() && values.contexts.is_empty()));
        let value = when::<i32, i32, i32, &'static str>(&tasks, [When::Value(9)]);
        assert_eq!(value.state(), State::Resolved);
        let source = D::new();
        source.resolve(3);
        let result = when(&tasks, [When::Promise(source.promise())]);
        assert_eq!(result.state(), State::Resolved);
        assert!(tasks.is_empty());
    }

    #[test]
    fn when_rejects_with_the_first_observed_failure_and_its_context() {
        let tasks = TaskQueue::new();
        let first = D::new();
        let second = D::new();
        let result = when(
            &tasks,
            [
                When::Promise(first.promise()),
                When::Promise(second.promise()),
            ],
        );
        second.reject_with("network", 4);
        first.reject_with("ignored", 5);
        assert_eq!(result.state(), State::Rejected);
        result.fail_with(|context, reason| {
            assert_eq!(context.copied(), Some("network"));
            assert_eq!(*reason, 4);
        });
        // Source loops backwards: with two already-rejected inputs, the second
        // is observed first while installing the subscriptions.
        let result = when(
            &tasks,
            [
                When::Promise(first.promise()),
                When::Promise(second.promise()),
            ],
        );
        result.fail(|reason| assert_eq!(*reason, 4));
    }

    #[test]
    fn continuations_do_not_retain_a_dropped_scheduler() {
        let tasks = TaskQueue::new();
        let weak = Rc::downgrade(&tasks.tasks);
        let source = Adopting::new();
        let returned = Adopting::new();
        let returned_promise = returned.promise();
        let _child = source
            .promise()
            .then_adopt(&tasks, move |_| returned_promise.clone());
        source.resolve(1);
        assert_eq!(tasks.pending(), 1);
        drop(tasks);
        assert!(
            weak.upgrade().is_none(),
            "internal queued closures must not own their scheduler"
        );
    }

    #[test]
    fn multi_argument_payloads_are_preserved_without_dynamic_js_values() {
        let tasks = TaskQueue::new();
        let source = Deferred::<(i32, String), (), ()>::new();
        let forwarded = source.promise().then_identity(&tasks);
        source.resolve((3, String::from("four")));
        tasks.drain(10);
        forwarded.done(|value| assert_eq!(value, &(3, String::from("four"))));
    }
}
