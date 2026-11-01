# Hull JavaScript and native input

Hull starts one idle Tokio local task with its event loop. It shares Hull's
thread; there is no additional OS worker. The standalone QuickJS crate has an
opaque native value boundary and no dependency on the old workbench/experiments.

Page extraction only stores script bytes and references. `Runtime::run()` remains
an explicit execution capability with no call from Hull's navigation, UI events,
page loading or shell. Input never creates a VM or activates dormant scripts.
Navigation drops the context, globals, promise jobs, listeners and queued input.
The new page requires a new explicit run request. Closing Hull drops the task.

## Native input bridge

The one-way path is Winit's opt-in captured input copy → owned, bounded Rust
messages → native JSON parsing/function call → callbacks in the Tokio task.
JSON payloads are never interpolated as JavaScript source. The receiver is pinned
as a native JS value and its temporary global name is removed.

The only actor-installed endpoint is `hullInput`. **There is no DOM connection:**
no `document`, `window`, nodes, selectors, hit testing, scene commands, layout,
painting, or SVG access. Callbacks may change their own JavaScript business state;
they have no path to change Hull's parsed document, rendered frame or SVG snapshot.

After a future explicit run, page code can subscribe with:

```js
hullInput.addEventListener('pointer', event => {
    globalThis.lastPointer = {device: event.device_id, raw: event.raw};
});
hullInput.addEventListener('keyboard', event => {
    globalThis.lastKeyboard = event;
}, {once: true});
```

`removeEventListener(kind, callback)` removes a listener; `'*'` subscribes to every
kind. `snapshot()` returns immutable `{lastEvent, received, resetCount}`. Events,
raw records, nested arrays and snapshots are frozen. Listener errors do not skip
later listeners; the first error is reported through the actor. No successful
input callback is logged to the UI. Promise jobs run between input callbacks.

Version 1 envelopes carry `kind`, native `window_id`, decimal-string generation,
revision and sequence. Pointer/keyboard/pan records preserve their native device
ID, complete endpoint, flags, combo/virtual-cursor fields and raw bytes. Keyboard
sequences/times are decimal strings. Coordinates stay tagged `ui4-native`; the
bridge performs no pixel/DotM or CSS conversion. Keyboard text is UTF-8 validated;
malformed bytes remain available in `raw` and are marked invalid. Named-key NUL
markers are not reported as committed text. Keyboard combo IDs are not invented.

Native pointer/keyboard/pan copies are authoritative. Standard translated key,
pointer and wheel callbacks are not forwarded a second time. Focus, resize, IME
and modifier changes form supplemental messages; captured raw records are drained
before them so state notifications cannot overtake their source records. Native
queue order is the backend's delivery order, not a global hardware chronology.
Draining copies does not consume or change Hull's existing Winit UI callbacks.

The bridge admits 256 messages, at most 16 KiB serialized each, and dispatches at
most 32 per task turn under a cooperative time allowance. During page initialization
it waits for the initial scripts/jobs. Inactive-page and stale-generation input
is discarded, never replayed into a later page. Overflow discards the backlog and
reports an accumulated `input-reset` before subsequent records. A bridge-wide
reset has `scope: 'all-windows'`, `window_id: null`, and `reset_all: true`; a native
window queue reset retains that window's ID. Consumers must clear held-state
assumptions and resample later input after reset. Malformed/oversized records are
rejected explicitly; Hull logs the rejection without logging typed text.

## Execution limits and scope

Explicit execution evaluates one source per turn; data MIME types are skipped,
external references remain unfetched, and modules have no import loader. Jobs
run before the following source, at most sixteen per batch. There are no console,
filesystem, network or timer bindings. Unhandled Promise rejection tracking is
not installed yet.

The engine defaults to a 32 MiB heap, 1 MiB stack and 25 ms cooperative execution
interrupt, with at most 1 ms extra cooperative diagnostic time. Parsing, GC and
native builtins can overrun it: this is not a hard UI-latency guarantee or an OS
sandbox. Interrupted evaluation aborts rather than resuming on the next turn.

Host fixtures in `tools/js-tests` compile the production actor and Winit adapter.
The engine crate has separate native tests; `tools/layout-tests` checks the static
rendering path. Built-in packaging uses `../tools/build.py --app hull` and does
not launch Hull.
