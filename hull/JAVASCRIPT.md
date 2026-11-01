# Hull JavaScript and native input

Hull starts one idle Tokio local task with its event loop. It shares Hull's
thread; there is no additional OS worker. The standalone QuickJS crate has an
opaque native value boundary and no dependency on the old workbench/experiments.

Page extraction only stores script bytes and references. Navigation and page
loading never execute them. The terminal's explicit `:run` calls `Runtime::run()`
once for the collected sources. Input never creates a VM or activates dormant
scripts. Navigation drops the context, globals, promise jobs, listeners, queued
input and queued pastes. The new page requires explicit execution again. Closing
Hull drops the task.

## Terminal console

Hull uses a small Crossterm terminal companion with the existing TRUEOS terminal
lease handoff. It displays a bounded output tail and one prompt. Paste an HTTPS
URL and press Enter to navigate. There are no panels or Ratatui dependency.

- `:scripts` lists collected inline/external sources, node IDs, types, byte counts
  and resolved origins. External sources are labeled unfetched.
- `:run` requests the collected inline JavaScript once for this page.
- `:paste` starts an inert multiline JavaScript buffer; type `:end` on its own
  line to execute it, or `:cancel`/Ctrl-C to discard it. Bracketed paste preserves
  script text literally: command-looking lines inside the pasted payload do not
  submit commands. The whole script is rejected if it exceeds 64 KiB. At most
  eight pastes can await execution.
- Esc restores the terminal and parks it back at the shell. `tui` re-enters the
  same console while Hull continues processing its browser window. Ctrl-Q exits.

Pasted sources require a loaded page and share its VM and globals. They enable
the input endpoint but do not request the page's collected scripts. Requested
page sources and their Promise jobs finish before queued pastes; each paste runs
as one classic script per turn, with Promise checkpoints between sources.

The text console supports `log`, `debug`, `info`, `warn`, `error`, and `assert`.
Arguments are joined with spaces; `%s`, `%d`, `%i`, `%f`, `%o`, `%O`, and `%%`
provide basic formatting. Objects use JSON text where possible, with string
fallback for cycles/unsupported values. A false assertion prints an error;
conversion errors remain ordinary JavaScript errors. There are no tables,
groups, inspector, debugger or arbitrary shell commands.

Console calls queue owned text in the native engine rather than writing the
terminal inside guest execution. Each drain admits 64 records of at most 4096
UTF-8 bytes, preserving character boundaries and reporting excess records.
Hull drains after sources, Promise batches and input batches, tagging output
with the page generation. The terminal retains 256 lines and strips terminal
control characters; long visible lines are clipped to the terminal width.
Native diagnostics and script errors use the same output path.

Small synchronous helpers are `performance.now()` (monotonic milliseconds since
VM creation), `atob()` and `btoa()` (Base64 and binary byte strings, with invalid
input errors named `InvalidCharacterError`). They add no DOM objects. QuickJS
already provides `Date`/`Date.now()`, JSON, URI helpers and standard language
builtins. There is no timer scheduler.

## Native input bridge

The one-way path is Winit's opt-in captured input copy → owned, bounded Rust
messages → native JSON parsing/function call → callbacks in the Tokio task.
JSON payloads are never interpolated as JavaScript source. The receiver is pinned
as a native JS value and its temporary global name is removed.

The native input endpoint is `hullInput`. **There is no DOM connection:**
no `document`, `window`, nodes, selectors, hit testing, scene commands, layout,
painting, or SVG access. Callbacks may change their own JavaScript business state;
they have no path to change Hull's parsed document, rendered frame or SVG snapshot.

After explicit execution, script code can subscribe with:

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

The native frame includes a fixed 32-unit chrome row above the content viewport
(initially 1280×720 inside a 1280×752 frame). Raw input still describes the whole
frame; the bridge does not silently subtract that row. Chrome and its favicon
are outside HTML layout and the saved page SVG, and are not exposed to JavaScript.

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
run before the following source, at most sixteen per batch. There are no
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
