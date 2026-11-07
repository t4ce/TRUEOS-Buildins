# Hull JavaScript and native input

The TRUEOS application and HTML parser use `core` and `alloc`. Its worker runs
the native Tokio runtime; shared locks and monotonic deadlines use the native
execution services. Desktop fixtures retain their host clock and locks. Browser
script, DOM, navigation and resource policies share the same application code.

Hull submits one actor through the existing `trueos::worker::spawn` native
service lane. Its current-thread Tokio runtime, LocalSet, QuickJS realm and
Rc-backed DOM are constructed and destroyed inside that lane. Owned commands,
reports and frozen snapshots cross the UI boundary; QuickJS and Rc handles do
not. Tokio notifications wake input, navigation, pasted scripts and media
receipts; timer deadlines wake the lane independently of UI polling. Resource
tickets are checked at bounded intervals, and idle waits check native worker
cancellation at least every 50 ms. The standalone QuickJS crate has an
opaque native value boundary for browser callers and no dependency on the old
workbench/experiments. Its optional `raw` feature also serves Codex Code Mode,
which owns separate contexts, tool callbacks, cancellation and job pumping.
Both hosts compile the same pinned sources and C ownership wrappers; Code Mode
does not install Hull’s DOM or browser globals.
QuickJS also owns the four byte/string search and comparison primitives used by
its C engine. Native calls bind to these local algorithms; they use no kernel
compatibility service. The remaining math/assertion helpers are freestanding.
Its C sources and adapter use QuickJS's full-pointer value representation in
both host tests and TRUEOS. The upstream 47-bit NaN box cannot retain TRUEOS's
canonical high-half heap pointers; a native pointer round-trip regression checks
this without dereferencing synthetic addresses.

After rp5 completes an HTML parse, Hull admits its collected inline JavaScript
for a finite initialization pass. External script references use the bounded
host resource loader; there is no guest fetch or filesystem API. Navigation drops the context,
document wrappers, Promise jobs, listeners, queued input and queued pastes.
Closing Hull signals cooperative shutdown without waiting on the UI thread. The lower-level `replace_page` path remains inert
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
  the actor runs. This also bounds a long collection of tiny sources or a
  self-replenishing microtask queue. At the cutoff, the first-render document is
  captured; remaining sources and jobs may continue using the remaining active
  budget. Capturing the SVG does not discard the actor or its live DOM.

When admitted work finishes or a bound is reached, Hull clones the current
document once for layout. CSS extraction runs on that frozen clone, so admitted
text, attribute and authored style changes participate in the first frame and
saved layout SVG. Later mutations affect the live script-visible document only:
they do not request layout, redraw, update native hit boxes or rewrite the SVG.
Native button hover and scrolling continue to operate on the rendered snapshot.

Guest calls are serialized within the native lane while the UI remains free
to process input and render. Navigation replaces the lane-owned realm before
admitting the new page revision; closing the lane discards pending work.
Each guest call retains its 25 ms cooperative interrupt. Interrupted calls abort
rather than resume; completed mutations remain. These bounds are cooperative:
parsing, GC, native operations and cloning can overrun a checkpoint. Rust selector
walks have their own finite resource limits. This is not an OS sandbox or a hard
250 ms UI-latency guarantee.

## URL and query parameters

Active page and console-only realms expose `URL` and `URLSearchParams`. Parsing,
relative resolution, browser-style component setters, origins and form query
encoding/decoding reuse Hull's existing Rust `url` crate. QuickJS retains object
identity, Web IDL string coercion, list mutations, callbacks and live iterators.
There is no persistent native URL-object registry.

URL provides href/protocol/username/password/host/hostname/port/pathname/search/
hash accessors, origin, toString/toJSON, canParse and parse. Its same-object
searchParams view tracks href/search changes in both directions. URLSearchParams
accepts strings, records and name/value iterables, with size, append/delete/get/
getAll/has/set/sort, forEach and live entries/keys/values. USVString conversion
replaces lone surrogates; sorting uses stable UTF-16 key order. Blob object-URL
registration/revocation is not provided.

These constructors do not navigate, fetch, read files or imply window.location.
UTF-8 string inputs/serialized outputs and aggregate query data are bounded to
128 KiB per operation, with at most 4,096 query pairs; excess throws RangeError.
The shared QuickJS allowance applies; native parsing is bounded cooperatively,
not preempted by a separate thread.

Host fixtures cover components, invalid setters, Unicode/IDNA/IPv6, live identity,
iterators, console-only installation and dynamically loaded modules using
import.meta.url. A development oracle forces the pinned core-js 3.51.0 fallback
and compares 153 source cases against the production binding. This does not
constitute full web-platform conformance.

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
- Document/Element `getElementsByTagName`, with live `HTMLCollection` results,
  wildcard matching, indexed access, `item`, `namedItem` and iteration. These
  traverse the same rp5 arena and retain the gateway's resource bounds.
- Document `createElement`, `createTextNode`, `createComment` and
  `createDocumentFragment`; Node `appendChild`, `insertBefore` and `removeChild`.
  Creation produces detached nodes. Insertion moves existing handles, expands
  fragment children, and checks ancestry/document hierarchy before mutation.
  All allocations share the 4,096-node and 4 MiB DOM mutation allowances.
- `Text` and `Comment` wrappers/constructors use real rp5 character nodes, with
  `CharacterData.data` and UTF-16 `length`. `Node.cloneNode(deep)` copies detached
  nodes, attributes, descendants and template contents within those same quotas;
  it does not copy listeners or JavaScript expando properties. Cloning a Document
  remains explicitly unsupported. Element
  `insertAdjacentElement` uses the existing validated insertion operations for
  its four standard positions; these are DOM APIs rather than jQuery aliases.
- HTML link `href`, `rel`, `as` and `crossOrigin` reflection. Document URL and
  live `baseURI` resolve href values through Rust's URL parser and the first base
  element. Script-created stylesheet links participate in CSS collection from
  the frozen initial document. Explicitly inserted script elements are queued on
  the existing actor, use the bounded host loader and receive trusted load/error
  events through the existing EventTarget. HTMLScriptElement reflects src, type
  and text, with onload/onerror handlers. Classic scripts set currentScript;
  removal and reinsertion do not rerun a started script. Queue admission does not
  reproduce the browser's full synchronous insertion/async/defer ordering model.
  Module imports retain the existing host-managed loader. Custom element
  registries and a complete template model are not admitted.

`document.implementation.createHTMLDocument(title)` creates an inert document
with its own doctype, html, head and body in the existing rp5 arena. An optional
title is text, never markup. The page counts as document one: at most nine more
can be created per navigation. Further attempts throw `QuotaExceededError` and
emit one console warning for the page. Detached documents have no window,
current script, resource loading or execution context; queries stay within their
own tree. Native node creation, cloning and cross-document insertion retain or
adopt `ownerDocument` as appropriate. They do not enter the rendered page unless
nodes are explicitly moved into its tree. Element.innerHTML parses rp5 fragments
with the element's parsing context and serializes actual children. Template
markup uses its content fragment. Replacement is validated against the existing
string/node budgets before changing the live tree. Parsed script elements and
clones of started scripts stay inert. Node.childNodes is a cached live NodeList;
querySelectorAll retains its static NodeList behavior.

Document.createEvent supports Event/Events/HTMLEvents and CustomEvent through
the existing Event wrappers. Returned events require initEvent/initCustomEvent
before dispatch. Unsupported event interfaces raise NotSupportedError.

Native jQuery accepts `$(function ($) { ... })` and `$(document).ready(fn)`.
Callbacks use the existing DOM-ready signal, run asynchronously in Promise jobs,
receive jQuery and use the page document as `this`. Late registrations also run;
each callback runs once, within the same page execution budget. A failing
callback is reported to the console without suppressing the other callbacks.

When Hull attaches its native window producer, `window` and `self` expose the
existing page global, with the same `document`, console and script globals.
`innerWidth` and `innerHeight` read the live `Viewport::extent()` also passed to
layout. Creation and `SurfaceResized` update that state after subtracting Hull's
chrome: the initial 1280x752 native frame yields 1280x720 content, and a 640x400
resize yields 640x368 content. No guest callback executes during a native update.
`window` keeps its identity; `self` and the inner dimensions have standard
replaceable property behavior if page code assigns them.

Window-backed pages expose diagnostic-only `location`/`document.location` and
`window.open`. Location getters retain the actual current URL; component writes,
assign/replace/reload and open only log a blocked navigation target. Relative
URLs use the document base. The first three attempts per navigation are logged,
the fourth emits one "navigation log exceeded" message, and further attempts
are silent. No URL is fetched or executed, no window opens, and no document URL
changes. `window.open` returns null. `navigator` is a separate browser-information
API; it is not a navigation operation.

Window-backed realms expose a virtual Navigator, also available through
clientInformation. Its userAgent is `TRUEOS net-fetch`, matching the current
kernel HTTP page/media identity, and platform is `TRUEOS`. PluginArray and
MimeTypeArray are allocated lazily on first access and retain their identity.
Both collections are empty, with length/item/namedItem/iteration and the legacy
plugins.refresh method. pdfViewerEnabled is false. These values advertise no
Flash, PDF viewer, decoder or MIME capability. serviceWorker, userAgentData,
mediaDevices, hardware concurrency, online state and other unimplemented
Navigator services remain absent. Navigation creates a fresh virtual object;
DOM-only fixtures without a window producer do not acquire a Navigator.

This does not add a general Window implementation or child browsing contexts.
DPI, outer/screen geometry and focus bindings
remain absent. In particular the current Winit backend's `scale_factor()` is a
constant 1 and `has_focus()` is a false placeholder, so neither is reexported as
measured browser state. DOM-only fixtures without a native producer do not gain
window globals or fabricated dimensions.

Selectors use the CSS parser and shared matcher, supporting common type, ID,
class, attribute, combinator, structural and `:scope`/`:is`/`:where`/`:not`
queries. Malformed selectors raise `SyntaxError`; unsupported valid features
raise `NotSupportedError`. Bounds raise `QuotaExceededError`. There is no
form-control value model, computed layout or observer API.
SVG `className` is explicitly unsupported
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
builtins.

`setTimeout(callback, delay, ...args)`/`clearTimeout(id)` and
`setInterval(callback, delay, ...args)`/`clearInterval(id)` share one monotonic
deadline queue on the native QJS worker. Its Tokio task waits for notifications
or the next deadline; timers do not depend on Hull UI polling. Callbacks retain
arguments in QuickJS, receive the page global as `this`, and run one per worker
step with a Promise checkpoint before the next callback. Ready collected
scripts have priority. Delays are minimum waits, converted to signed 32-bit milliseconds;
negative results become zero, and nested chains use the HTML 4 ms clamp
once nesting exceeds five. Late deadlines do not generate catch-up bursts:
intervals rearm after callback completion using the same ID. Both clear
functions cancel either kind, including an interval cancelling itself.

Each realm admits at most 256 pending timers. Full queues throw
QuotaExceededError. String/eval handlers are explicitly unsupported and throw
TypeError. Cancellation removes the deadline and
callback. Navigation discards the whole queue. Callback faults are reported
without discarding other timers. All callback work spends the same 250 ms page
budget; timers never renew it or extend the first-render cutoff. Future timers
do not hold the initial snapshot open; subsequent changes stay in the live DOM
without requesting another layout or updating saved artifacts.

## Inline style admission

Elements expose a lazy, same-object CSSStyleDeclaration through style. Camel-case
properties (backgroundColor, cssFloat), hyphenated names, cssText,
getPropertyValue/getPropertyPriority/setProperty/removeProperty and
length/item/iteration use the existing Lightning CSS parser and real rp5 style
attribute. Reads are inline declarations, not computed layout or computed style.

The first valid script write to a CSS field on an element commits for that
navigation. Later assignments/removals of that field are silent no-ops; each
element has independent locks. Existing HTML declarations remain editable until
the first script write. Invalid values/priorities do not latch a field.
Shorthands lock their longhands; a shorthand touching any locked longhand is
ignored in full. cssText, style attribute changes and jQuery attribute writes
preserve frozen fields while admitting new ones. Custom property names retain
their case and use setProperty/getPropertyValue. all is conservatively blocked
after any field is frozen and, when admitted first, locks subsequent CSS writes.
These write-once semantics deliberately restrict ordinary mutable CSSOM behavior.

Fields share the existing DOM byte quotas, plus a 4,096-field lock allowance
per navigation and 256 declarations per inline block. Navigation resets locks;
clones are separate nodes. Accepted changes before the initial snapshot reach
layout/SVG. Later live changes do not trigger another layout or rewrite the SVG.

## Separate video playback

HTMLVideoElement wrappers expose src, currentSrc, paused and play(). A play
request selects the element's src or its first direct source[src] child,
resolves it against baseURI, and admits only a bounded HTTPS URL. This is a
handoff to the existing native video player, not video embedded in Hull's layout.
No player layout, pixel copies, shader, audio control or new decoder is added.

Each Hull navigation has one newest request slot shared by all video elements.
The native service keeps one active session and one newest preparation candidate
per owner. New requests supersede preparation, without pausing active playback.
Only successful publication of the candidate's first converted frame retires
the old player. Preparation failures leave it playing. The existing global
three-session decoder cap includes these candidates; admission waits up to 30 s
for a slot. Playback belongs to the native service and survives Hull closing.

play() returns a Promise pending until a host first-frame receipt; superseded
requests reject with AbortError and failed handoffs with NotSupportedError.
Only confirmed playback emits playing. Old-generation receipts cannot reach a
new page. The bridge does not implement autoplay, pause/seek, duration/metadata,
canPlayType, a full media event/state machine or arbitrary player libraries.
Explicit page play calls and the bounded media discovery paths below use the
same native handoff. A queued handoff, first-frame publication or preparation
failure is also logged to Hull's console, including requests without a play
promise.

Offline sandbox replay records video_requests in report.json and rejects native
playback explicitly; it never downloads media or opens a native player.

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
filesystem or general-purpose network bindings. Unhandled Promise rejection tracking is
not installed yet.

The engine defaults to a 32 MiB heap and 1 MiB stack. Its optional cumulative
allowance charges native callbacks and exception diagnostics as well as guest
evaluation. Diagnostics get at most 1 ms and cannot renew an exhausted budget.

Host fixtures in `tools/js-tests` compile the production actor and Winit adapter.
The engine crate has separate native tests; `tools/layout-tests` checks the static
rendering path. Built-in packaging uses `../tools/build.py --app hull` and does
not launch Hull.

`tools/sandbox/sandbox.py` captures HTML/CSS once and replays this same production
actor/DOM/layout code on the host, without a kernel build, boot, VM or GPU. It
exports exact script bytes, per-source traces and optional diagnostic SVG; node
isolation, local replacements and watch mode support iterative investigation.
`replay --post-ms 1000` keeps observing the actor for one second after export.
The saved SVG and storage snapshot stay fixed; later script reports, dynamic
sources and video URL requests appear in the trace/report. This does not extend
the page's cumulative active execution budget.
See `tools/sandbox/README.txt` for commands and explicit experiment flags.

`document.cookie` reads navigation-local `name=value` pairs separated by `; `.
The setter converts its argument to a string, replaces an existing name, and
supports deletion with `Max-Age=0` or a negative Max-Age. Attributes are never
returned as pairs. Each navigation starts empty; saved diagnostic cookies are
never loaded and cookies are never transmitted with resource requests. This
small admission does not implement domain/path scoping, expiration clocks,
Expires dates, Secure/HttpOnly enforcement or SameSite behavior.

The per-navigation cumulative cookie budget is 1 MiB: every setter attempt
charges its JSON-encoded UTF-8 input bytes (including quotes/escapes) plus 32
bytes for entry overhead, including
replacements, deletions and invalid inputs. A write exceeding the remaining
budget latches the jar against all further writes. Reads remain available.
Cookies, `localStorage` and the read-only `sessionStorage` view share one
Rust-owned navigation-local key/value box. Internal keys are prefixed
`cookie:`, `localStorage:` and `sessionStorage:`; scripts never see
those prefixes, and identical public names do not overwrite each other.
`localStorage` provides getItem/setItem/removeItem/clear/key/length and named
property reads, writes and deletion. Missing getItem returns null. Web Storage
setItem throws QuotaExceededError on exhaustion; cookie setters remain silent.
Removing localStorage keys or clearing them does not renew the cumulative budget.
Storage strings retain the gateway's Unicode scalar limitation.

`storage.json` is written once beside `layout.svg`, containing the shared box
as prefixed key/value pairs captured when that SVG is prepared. It replaces the
previous cookie-only diagnostic file. No saved data is restored. Later scripts
can change live entries without refreshing this initial snapshot.

`sessionStorage` uses the same Storage interface and native box with its own
prefix, but is read-only by Hull policy: setItem, property assignment,
defineProperty, removeItem, deletion and clear silently leave stored values
unchanged and do not spend the write budget. Missing entries read as null through
getItem or undefined through named access. The fresh navigation box supplies no
prior-session or other-tab entries. Reading this view performs no navigation,
network requests or tab operations.


## URL-only player admission

KVS/KT `kt_player.js` entry points use a small compatibility adapter instead of
constructing the browser/Flash player UI. The page's normal kt_player call
supplies video_url; the adapter passes that exact source through the existing
HTMLVideoElement.play/native vid handoff. It does not decode obfuscated KVS
sources, invent a source, or claim successful playback before a native receipt.
Recognized Google/reCAPTCHA scripts are skipped without synthesizing success.

The same worker can inspect media subdocuments without embedding them. It
recognizes ordinary video/source markup, VideoObject contentUrl in JSON-LD
(including arrays and @graph), and iframe sources labelled as video/player by
title, id, name or class, then follows at most nine distinct documents and three levels. Each
HTML response is limited to 256 KiB and uses the existing bounded resource
loader. A disposable, same-thread QJS realm can evaluate up to eight short
inline scripts and the explicit Play button handler. Its small MDCore adapter
captures a player's wurl assignment through the existing video.src/play sink;
packed inline JavaScript is evaluated normally. External scripts and frame
timers are not run. Location assignments become same-origin media
inspection requests; window.open, top-level navigation and layout stay inert.
Inspection execution spends the remaining page allowance, with a 25 ms local
cap. Finding a URL stops inspection; subsequent explicit page play requests
still use the existing latest-request policy. These temporary trees are not
persistent page DOMs or additional windows/workers.

Media subdocument fetches carry their actual parent document URL through the
existing bounded download tickets. The host applies a same-origin full URL or
cross-origin origin-only Referer, without fragments or credentials, on every
redirect. This does not expose arbitrary request headers to JavaScript.
Offline replay still needs the corresponding captured HTML responses imported
into capture.json; replay itself never contacts a server.

The native backend supports finite, unencrypted HLS byte ranges that index one
contiguous fragmented MP4 resource, using the existing MP4/AVC demux. Multiple
fragment resources, encryption and discontinuities remain unsupported. Native
HTTPS MP4 playback currently downloads the complete file before demux and first
publication, with a 512 MiB limit. A discovered URL therefore does not imply
that a Vid window can already open. Larger files need a bounded range reader;
increasing this limit also increases whole-file buffering in the kernel heap.
