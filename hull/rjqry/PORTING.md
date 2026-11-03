# Source map and compatibility boundaries

`UPSTREAM.json` fingerprints the supplied archive. Typed Rust translates native
data/control algorithms; a host/binding must supply the browser and language
semantics below. Unsupported paths must not be exported as successful stubs.

| Upstream source | Native code / remaining boundary |
| --- | --- |
| `core.js`, `core/toType.js`, `core/isArrayLike.js` | `collection`, `utilities`, `value`; owned data/typed callbacks, no arbitrary JS object heap |
| `core/access.js` | Typed access/callback setters; dynamic JS overload/property-map dispatch needs binding |
| `core/camelCase.js`, `core/stripAndCollapse.js` | `data::camel_case`, `utilities::strip_and_collapse` |
| `core/init.js`, `core/isObviousHtml.js` | Collection construction/query contracts; HTML strings/creation/property bags need host |
| `core/nodeName.js`, `core/isAttached.js` | Actual node names/document membership need DOM host |
| `core/DOMEval.js`, `core/parseHTML.js`, `core/parseXML.js`, `core/ready*.js` | Unimplemented: actual parser/evaluation and lifecycle delivery |
| `callbacks.js` | `callbacks`, including upstream option matrix |
| `deferred.js`, `deferred/exceptionHook.js` | `deferred`; caller queue and native Promise adoption, foreign thenables/exception/console/stack hooks need binding |
| `data.js`, `data/Data.js`, `data/var/*` | `data`; owner admission/private-user stores and real data attributes need host wiring |
| `queue.js`, `queue/delay.js` | `queue`; owner/type maps, aggregate element queue promises and real delay clock/cancellation need adapter |
| `traversing.js`, `traversing/findFilter.js`, `traversing/var/*` | `collection`; queries/seed filtering/unique ordering delegated to Dom |
| `selector/escapeSelector.js` | `utilities::escape_selector` |
| `selector.js`, `selector-native.js`, other `selector/*` | Host contracts; tokenizer/compiler, caches, jQuery pseudos and browser probes are not translated |
| `attributes/attr.js`, `attributes/prop.js`, `attributes/classes.js` | Native iteration, null/false/aria, property fixes, class tokens; actual XML/attributes/properties and browser hooks need host |
| `attributes/val.js` | Unimplemented: option/select/checkable/control-value hooks need form semantics |
| `manipulation.js`, `manipulation/domManip.js` | Native insertion order/cloning/self-selection exclusion/cleanup/removal dispatch; no HTML fast path or script execution |
| `manipulation/buildFragment.js`, `getAll.js`, `setGlobalEval.js`, `_evalUrl.js`, wrap maps | Real parsing/descendant cleanup/script lifetime/transport need host |
| `wrap.js` | Unimplemented: wrapper creation/first-child insertion require parser/DOM adapter |
| `event.js`, `event/trigger.js` | `events` registry/dispatch/portable mappings; native listeners, inline on* callbacks, window bubbling, default actions, native focus and beforeunload return-value writeback need host; ordinary namespace tokens only |
| `serialize.js` | `serialize` param/form pairs; serializeArray control discovery, disabled fieldsets and val hooks need form host |
| `css/cssCamelCase.js`, `css/isAutoPx.js`, numeric regex vars | `style` |
| `css/adjustCSS.js` | Same-unit endpoint arithmetic; measured iterative unit conversion needs host |
| `css.js`, `curCSS.js`, `finalPropName.js`, `showHide.js`, other `css/*` | Unimplemented: computed style, vendor probing, box measurement, visibility, writes and browser probes |
| `effects/Tween.js`, portable `effects.js` | `effects` sampling/easing/speed/queue normalization; property hooks, animation prefilters, scheduling, DOM show/hide/completion dispatch need host |
| `effects/animatedSelector.js` | Needs animation registry/selector adapter |
| Portable helpers in `ajax.js` | `ajax` registry algorithms, settings merge, response selection/conversion; host supplies callbacks/MIME tests |
| `ajax.js` request lifecycle, `ajax/*` | Unimplemented: jqXHR lifecycle, URL/origin policy, HTTP/XHR/binary/script/JSONP/load, cache/header state, real timeout/abort/network events |
| `dimensions.js`, `offset.js` | Unimplemented: layout/scroll/window geometry |
| `deprecated.js`, `deprecated/*` | JS alias/global compatibility layer not installed |
| `exports/*`, `jquery.js`, `wrapper*.js`, umbrella imports, `var/*` | JS build/global/module plumbing; portable constants are colocated with native algorithms, browser objects stay host-owned |

This map includes untranslated modules deliberately. The crate is a native
foundation for the portable surface, not a complete drop-in jQuery runtime.
Hull now admits the DOM-focused subset described in README.md through its own
rp5/QuickJS adapter. The crate retains zero dependencies; remaining integration
is separate, and no guest network/filesystem API was added.

## Verification scope

Rust unit tests exercise reentrancy and collection-host interactions. Differential
fixtures execute original algorithms for data/core collections, callbacks, queues,
Deferred ordering, event traces, CSS/effects arithmetic and Ajax helpers. Browser
DOM and array-init fixtures are explicit test adapters. Original full browser test
suites, browser-specific probes and native transports have not been run.

Value cannot express accessors, proxies, arbitrary host objects, reference
identity, cycles or unpaired UTF-16. The JSON reader distinguishes invalid syntax,
valid but unrepresentable strings, and resource limits. data-* syntax failures keep
the original string; representation/resource failures remain explicit errors.
String encoding uses Unicode scalar values; generic native equality follows the
caller's Rust implementation. A JS binding must preserve language-level behaviors
instead of applying owned-data equivalence where object identity is required.
