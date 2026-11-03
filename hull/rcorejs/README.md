# rcorejs

A small native starting point with a **complete core-js 3.51.0 source reference**
in `core-js-3.51.0/`, beside Hull's `rjqry` folder. The reference contains all
4,355 files from the upstream tagged source archive, including packages, tests,
documentation and build sources. No npm install or upstream preparation was run.

`UPSTREAM.json` records the repository/tag, archive SHA-256 and source-tree
fingerprint. `LICENSE` retains the upstream MIT notice. The reference is kept
unchanged; the native code is outside it.

## Four translated functions

| Rust API | Original source under `packages/core-js/` |
| --- | --- |
| `abstract_ops::to_integer_or_infinity(f64)` | `internals/to-integer-or-infinity.js` |
| `abstract_ops::to_length(f64)` | `internals/to-length.js` |
| `string::is_well_formed(&[u16])` | `modules/es.string.is-well-formed.js` |
| `string::to_well_formed(&[u16])` | `modules/es.string.to-well-formed.js` |

The independent crate is `no_std` + `alloc`, forbids unsafe Rust and has **zero
dependencies**, including build/test dependencies. Numeric truncation operates
on IEEE-754 bits without std/libm; UTF-16 input preserves lone surrogates that
Rust `str` cannot contain. Repair preserves code-unit length and valid pairs.

These are typed algorithm bodies. JavaScript ToNumber/ToString, receiver checks,
Symbols/BigInts, getters, exceptions and allocation/execution budgets belong to
a future engine binding. Numeric signed-zero behavior follows the pinned
core-js helper, including negative zero from truncating negative fractions.

This crate is not linked into Hull or installed in QuickJS. It does not replace
existing engine builtins, bundle polyfills into page startup or provide a
JavaScript object heap. A future binding should use actual QuickJS values and
strings; the DOM JSON gateway cannot preserve arbitrary JS semantics.

Hull now exposes URL/URLSearchParams separately through its existing Rust `url`
dependency and production binding in `../src/javascript_url.{rs,js}`. Host JS
tests compare it with this reference's original URL polyfill. The URL parser was
reused, not translated into rcorejs.

## Validation and growth

```sh
python3 tools/check.py --target x86_64-unknown-uefi
```

The target must already be installed; omit it for host checks. `--target-dir`
can reuse an external Cargo cache. Validation runs offline with Rust, Python and
Node; there is no kernel build or VM boot.

The Node oracle executes the unchanged upstream numeric helpers. It disables
Node's two well-formedness methods before requiring the original modules, so
the **actual core-js fallback bodies** generate the UTF-16 expectations. It
checks 1,044 numeric inputs (including random IEEE-754 bit patterns, signed zeros,
NaNs, infinities and boundary values) and 1,623 UTF-16 sequences. Rust consumes
simple text fixtures without JSON/test dependencies.

To add a feature: identify its exact upstream module and typed/host boundary,
add the native body, extend the source oracle with meaningful edge cases, then
document the new API here. Keep JS coercion/identity concerns explicit. After
reviewing intentional changes, regenerate expectations with:

```sh
node tools/oracle.mjs --write
```

`python3 tools/reference.py` verifies the complete archive contents, including
files outside the translated subset. Its `--record` option is only for an
intentionally reviewed replacement baseline. The rest of core-js remains a
reference; this is deliberately not a full translation.
