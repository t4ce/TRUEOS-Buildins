# Hull JavaScript and native input

Hull starts one idle Tokio local task with its event loop. It shares Hull's
thread; there is no additional OS worker. The standalone QuickJS crate has an
opaque native value boundary and no dependency on the old workbench/experiments.

After rp5 completes an HTML parse, Hull admits its collected inline JavaScript
for a finite initialization pass. External script references remain unfetched;
there is no guest network or filesystem API. Navigation drops the context,
document wrappers, Promise jobs, listeners, queued input and queued pastes.
Closing Hull drops the task. The lower-level `replace_page` path remains inert
for navigation reset and explicit-execution fixtures.

## Initial document and rendering

The parsed rp5 document lives behind a single Rust-owned shared handle. Native
JSON calls connect lazily created JavaScript node wrappers to that same arena;
node identity is stable across queries and detachment. No second HTML parser or
independently maintained JavaScript tree is involved.

Initialization evaluates at most one source per task turn, with Promise jobs
before the next source (at most sixteen jobs per batch). There are two bounds:

- Each page VM gets **250 ms total active execution time**, shared by bootstrap,
  sources, Promise jobs, input callbacks, console pastes and native DOM calls.
  Idle time between turns does not spend this allowance. It is set once per
  page, not renewed for each script. Exhaustion refuses further guest work.
- The initial render waits at most a **250 ms wall-time window**, checked when
  the actor is driven. This also bounds a long collection of tiny sources or a
  self-replenishing microtask queue. At the cutoff, unstarted page sources are
  omitted from this pass. Remaining jobs may use the remaining active budget.

When admitted work finishes or a bound is reached, Hull clones the current
document once for layout. CSS extraction runs on that frozen clone, so admitted
text, attribute and authored style changes participate in the first frame and
saved layout SVG. Later mutations affect the live script-visible document only:
they do not request layout, redraw, update native hit boxes or rewrite the SVG.
Native button hover and scrolling continue to operate on the rendered snapshot.

The work is interleaved on Hull's thread, not parallel on another OS thread.
Each guest call retains its 25 ms cooperative interrupt. Interrupted calls abort
rather than resume; completed mutations remain. These bounds are cooperative:
parsing, GC, native operations and cloning can overrun a checkpoint. Rust selector
walks have their own finite resource limits. This is not an OS sandbox or a hard
250 ms UI-latency guarantee.

## Small standard DOM subset

- `document.getElementById`, `querySelector`, `querySelectorAll`,
  `documentElement`, `head`, and `body`.
- Element queries, `getAttribute`, `hasAttribute`, `setAttribute`,
  `removeAttribute`, reflected HTML `id`/`className`, `tagName`, `localName`, and
  `namespaceURI`. Authored CSS is accessible through the ordinary `style`
  attribute, not a fabricated computed style API.
- Node `textContent`, `nodeValue`, `nodeName`, `nodeType`, parent/child/sibling
  navigation, `ownerDocument`, and `isConnected`.
- Static `NodeList` results with indexed access, `item`, iteration and `forEach`.

Selectors use the CSS parser and shared matcher, supporting common type, ID,
class, attribute, combinator, structural and `:scope`/`:is`/`:where`/`:not`
queries. Malformed selectors raise `SyntaxError`; unsupported valid features
raise `NotSupportedError`. Bounds raise `QuotaExceededError`. There is no
`window`, form-control value model, `innerHTML`, computed layout, tree insertion
API, DOM event system or observer API. SVG `className` is explicitly unsupported
instead of approximating `SVGAnimatedString`. Unpaired UTF-16 surrogates are
rejected because rp5 stores Unicode scalar strings.

The native gateway admits at most 1 MiB of JSON per call, 256 KiB per assigned
DOM string, 4 MiB cumulative assigned bytes, 4096 added text nodes and 32768
query results. Selector queries also have input, nesting, node and traversal
limits. Unsupported features fail explicitly rather than returning guessed data.

## Terminal console

Hull uses a small Crossterm terminal companion with the existing TRUEOS terminal
lease handoff. It displays a bounded output tail and one prompt. Paste an HTTPS
URL and press Enter to navigate. There are no panels or Ratatui dependency.
An unattended launch without terminal support forwards its startup URL to a
terminal-capable NewShell3 Hull instance before creating a browser window.

- `:scripts` lists collected inline/external sources, node IDs, types, byte counts
  and resolved origins. External sources are labeled unfetched.
- `:run` is retained for explicitly staged inert pages. Normal HTML navigation
  already requests its initial inline sources; this command does not rerun them.
- `:paste` starts an inert multiline JavaScript buffer; type `:end` on its own
  line to execute it, or `:cancel`/Ctrl-C to discard it. Bracketed paste preserves
  script text literally: command-looking lines inside the pasted payload do not
  submit commands. The whole script is rejected if it exceeds 64 KiB. At most
  eight pastes can await execution.
- Esc restores the terminal and parks it back at the shell. `tui` re-enters the
  same console while Hull continues processing its browser window. Ctrl-Q exits.

Pasted sources require a loaded page and share its VM, live document, globals
and remaining execution budget. They do not alter the frozen render snapshot.
Requested page sources and their Promise jobs precede queued pastes; each paste
runs as one classic script per turn, with Promise checkpoints between sources.

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

The native input endpoint is `hullInput`. It does not dispatch DOM events or
perform hit testing. Callbacks may change JavaScript state and the live document
within the remaining budget, but have no scene commands, layout, painting or SVG
access and do not invalidate the rendered snapshot.

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

Execution evaluates one source per turn; data MIME types are skipped,
external references remain unfetched, and modules have no import loader. Jobs
run before the following source, at most sixteen per batch. There are no
filesystem, network or timer bindings. Unhandled Promise rejection tracking is
not installed yet.

The engine defaults to a 32 MiB heap and 1 MiB stack. Its optional cumulative
allowance charges native callbacks and exception diagnostics as well as guest
evaluation. Diagnostics get at most 1 ms and cannot renew an exhausted budget.

Host fixtures in `tools/js-tests` compile the production actor and Winit adapter.
The engine crate has separate native tests; `tools/layout-tests` checks the static
rendering path. Built-in packaging uses `../tools/build.py --app hull` and does
not launch Hull.
