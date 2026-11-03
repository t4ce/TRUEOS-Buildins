# rjqry

Native Rust translations of portable algorithms in the supplied `jquery-main`
tree. **`#![no_std]` + `alloc`, unsafe Rust forbidden, zero runtime, build or test
dependencies.** This is an independent Cargo workspace.

The crate itself does not install globals or supply a browser. Hull now connects
an initial DOM-focused subset through `../src/javascript_jquery.rs` and its
JavaScript binding, sharing the real rp5 arena. This remains a typed native
foundation, not a claim of complete jQuery compatibility.

## Hull admission

DOM-backed page realms expose `$` and `jQuery` with `fn.jquery` set to
`4.0.0-hull-native`. Selector construction/find, collection stacks/indexes/slices,
unfiltered parent/children/sibling traversal, text/attribute/class access and
mutation run through native rjqry. The binding preserves node wrappers and runs
JavaScript callbacks in QuickJS; `each` and callback setters are admitted. Static
`param` for data values and `escapeSelector` are native. `noConflict` releases the
global names. Node collections accept nodes, arrays and the existing NodeList;
they do not model arbitrary JS-value collections or fractional indexes.

The guest binding does not yet expose seeded filtering, HTML construction,
ready scheduling, form values, CSS/computed geometry, events, Deferred, queues,
data caches, effects, extend/plugin helpers or Ajax. Traversal selector arguments
and param function/cyclic/non-data values fail explicitly. Other library exports
remain available to Rust callers. Page scripts still run in QuickJS.

Native batches have a cooperative 20 ms work deadline as well as existing DOM
size/mutation limits; QuickJS retains its per-call slice and cumulative page
execution allowance. The native work deadline is checked at DOM work boundaries,
not an OS preemption mechanism. Batched mutations may be partial on failure.
The admission also bounds class operations to 16 KiB/256 tokens per incoming or
existing class list, and param conversion to 2,048 values, depth 32 and 128 keys
per object, so core inner loops stay bounded between deadline checks.
There is no new redraw, network, filesystem, timer or background task. Speedup
has not been benchmarked: request/result conversion still crosses the JSON host
bridge, while collection and mutation loops execute in Rust.

## Native code

| Module | Translated behavior |
| --- | --- |
| `collection` | Collection stacks/indexes/slices, each/map, find/filter/not/is, traversal/order, text/attributes/properties/classes, insertion/cloning/removal dispatch |
| `callbacks` | once/memory/unique/stopOnFalse, identity, reentrant add/remove/fire/lock/disable |
| `deferred` | Deferred/Promise state, progress, done/fail/always, queued then/catch/adoption, synchronous pipe, cycle rejection, ordered when |
| `queue` | Named queues, fx sentinel, next/dequeue, auto-start helpers, empty hooks, stop-hook lifetime |
| `events` | on/off/one, namespaces, identity, delegation, synthetic propagation, return-false/default prevention, propagation controls, portable special-event mappings |
| `data` | Cache/key algorithms, data-* coercion, bounded JSON reader, core camelCase |
| `serialize` | Nested/traditional params, form name/value pairs, encodeURIComponent encoding, form newline normalization |
| `utilities`, `value` | Owned values, property ordering/coercion, deep data merge, collection utilities, CSS selector escaping, selector-context detection, property-name fixes |
| `style` | CSS camelCase, default pixel units, numeric/relative tokens, same-unit arithmetic |
| `effects` | Speed/queue normalization, linear/swing easing, numeric Tween, caller-timed progress |
| `ajax` | Prefilter/transport registry algorithms, flat/deep settings merge, response selection and converter chains |

## Host boundaries

`Dom` connects collections to actual node handles. The backend supplies query and
matches, node relationships/kinds, document ordering and text. Optional operations
report `Error::Unsupported` by default: actual attributes/properties, HTML parsing
and serialization, insertion/cloning, event/data cleanup, iframe/template contents.
`is_xml` defaults to HTML behavior and must be overridden for XML backends.

Selector execution is a host operation. `Dom::filter` defaults to ordinary
selectors; positional/seed extensions need a backend implementation. `is` uses
document membership for context-sensitive selectors; `closest` follows upstream's
positional-selector exclusion. No second selector engine or HTML parser is hidden
in this crate.

Native listeners, event metadata and browser default actions require a host;
native focus synchronization is explicitly unsupported. Deferred work runs only
when the caller drains `TaskQueue`; queues return work for the caller to invoke.
There is no implicit thread, executor, clock, repaint or DOM-ready source.

Computed CSS, unit conversion, layout/geometry, visibility and actual animation
writes require a host. Ajax helpers perform no request: URL/origin policy,
HTTP/XHR, jqXHR lifecycle, script/JSONP transports, XML parsing and script evaluation
are unimplemented. [PORTING.md](PORTING.md) maps remaining work to source modules.

`Value` is owned data, not a JavaScript heap. Functions are typed Rust callbacks;
tuples/collections represent multiple arguments. Prototypes/accessors, reference
identity, cycles, foreign JS thenables and lone UTF-16 surrogates need a language
binding. Shallow copies are owned copies; `in_array` uses Rust `PartialEq`. Result
errors represent explicit rejection/throw channels; Rust panics are not caught.
JSON/serialization, array-like conversion and prefilter traversal expose resource bounds. Swing has a
documented numerical tolerance rather than bit-for-bit libm parity.

## Examples

```rust
extern crate alloc;
use alloc::vec;
use rjqry::{Value, serialize::param};
let data = Value::Object(vec![
    ("page".into(), 2.into()),
    ("search".into(), "a b".into()),
]);
assert_eq!(param(&data, false).unwrap(), "page=2&search=a%20b");
```

```rust
use rjqry::deferred::{Deferred, TaskQueue};
let tasks = TaskQueue::new();
let request = Deferred::<i32>::new();
let next = request.promise().then(&tasks, |value| Ok(value + 1));
request.resolve(41);
next.done(|value| assert_eq!(*value, 42));
tasks.drain(16);
```

## Offline validation

```sh
python3 tools/check.py --target x86_64-unknown-uefi
```

The target must already be installed; omit `--target` for host checks only.
`--target-dir PATH` reuses an external Cargo build directory. These checks use the
archive, Node, Python and Rust: no npm install, network, kernel build or VM boot.

The JavaScript oracles execute original source algorithms and compare recorded
results with Rust. Browser-facing imports are isolated where necessary; fixtures
document their typed/host scope. They are not a whole-browser conformance suite.
After inspecting an intentional source/behavior change, regenerate with:

```sh
node tools/oracle.mjs --write
node tools/event_oracle.mjs --write
node tools/style_oracle.mjs --write
node tools/ajax_oracle.mjs --write
```

`UPSTREAM.json` pins source, package and license by SHA-256. The archive's package
label is `4.0.0`; no commit identity was supplied, so no release revision is
inferred. Upstream remains unchanged. MIT notices are retained in [LICENSE](LICENSE).
