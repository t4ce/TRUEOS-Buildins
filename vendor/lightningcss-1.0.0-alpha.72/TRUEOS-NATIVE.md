# TRUEOS CSS engine

Hull enables `trueos-native`, `into_owned`, and `substitute_variables`, with
default features disabled. On the TRUEOS target the library uses `core` and
`alloc`; the hosted bundler, CLI, Node bindings, and source-map backends are not
part of this dependency closure.

The native build retains stylesheet and inline declaration parsing, selector
parsing and matching, CSSOM printing, recovery diagnostics, custom properties,
variable substitution, all property/value types, and color/calc math. Floating
point methods use `num-traits` with `libm`. Its parser and selector dependencies
also compile without `std`, including paths emitted by their procedural macros.

`stylesheet::ParserWarnings` is an `Rc<RefCell<Vec<Error<ParserError>>>>` on
TRUEOS. Parsing is synchronous and the collector does not need OS locks. Hosted
builds retain `Arc<RwLock<...>>`. Public `collections` aliases preserve the
native map types across API boundaries; ordered maps retain insertion order
with an explicit `FxHasher`. AHash has neither `std` nor runtime RNG enabled.

Hull always leaves `ParserOptions::css_modules` as `None`. The native filename
helpers operate on UTF-8 display names and omit host `Path`/`pathdiff` project
root normalization. These compiler-side CSS Modules helpers are outside Hull's
CSS behavior; native CSS Modules would need a separate path/hash contract.
Hosted filename normalization and CSS Modules behavior remain available.

The build tool discovers the versioned local vendor manifests automatically.
Standalone host fixture manifests need the matching crates.io path patches for
LightningCSS and its parser, selector, ownership, and procedural-macro crates,
including the core-only `precomputed-hash` trait crate.
