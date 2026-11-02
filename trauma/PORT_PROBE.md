# TRUEOS packing probe

On 2026-11-02, the initial, single authorized Blueprint probe failed before
Trauma or its transfer dependency graph compiled. A subsequent user request
authorized fixing that path blocker and one verification pack. The followup
removed the path blocker and stopped at a dependency feature conflict. No
transfer implementation port was performed.

## Integration prepared

- `Cargo.toml` declares the `trauma` binary alongside the existing library.
- `src/main.rs` calls the real `Download` and `Downloader` APIs. It keeps a
  current-thread Tokio runtime alive and, on TRUEOS/zkvm, claims the terminal
  lease, acknowledges its initial output, releases it to Shell, and waits for
  reentry into the same VM. Enter a URL to download into the current directory;
  an empty line returns terminal ownership to Shell. This wrapper is unvalidated
  on TRUEOS because packing stopped before compilation.
- `../TRUEOS-Blueprints/apps.json` registers `trauma` at `../trauma`, following
  the existing external-repository catalog convention.
- The ordinary Rust `main()` follows the Blueprint packer's generated `_start`
  convention. The warmed terminal flow follows `apps/ratatest/src/main.rs` and
  the typed lease API in `crates/trueos-v/src/vshell.rs`. No kernel policy or
  autostart change is necessary for catalog-based terminal launch.

## Direct dependency coverage

Existing vendor manifests were searched recursively under the sibling
TRUEOS-Buildins, TRUEOS-Blueprints, and TRUEOS `vendor` trees.

| Dependency | Existing local path | Version |
| --- | --- | --- |
| reqwest | `../TRUEOS-Buildins/vendor/reqwest-0.13.3` | 0.13.3 |
| tokio | `../TRUEOS-Buildins/vendor/tokio-1.54.1` | 1.54.1 |
| tracing | `../TRUEOS-Buildins/vendor/tracing-0.1.44` | 0.1.44 |
| tracing-subscriber (normal and dev) | `../TRUEOS-Blueprints/vendor/tracing-subscriber-0.3.23` | 0.3.23 |
| trueos (new target-specific integration API) | `../TRUEOS-Buildins/vendor/trueos` | 0.1.0 |

No existing vendor package was found for these original direct dependencies:
`form_urlencoded`, `futures`, `indicatif`, `reqwest-middleware`, `reqwest-retry`,
`reqwest-tracing`, `thiserror`, `bon`, `color-eyre`, `comfy-table`, `console`,
`opentelemetry`, and `rand`. Their manifest versions are exact registry pins;
this is partial local path coverage, not a fully vendored graph. No registry
source was copied into a vendor tree. Reqwest changes from 0.13.4 to the existing
0.13.3 fork; Tokio changes from 1.52.3 to the existing 1.54.1 fork.

## Single attempt and evidence

Working directory: `/home/t4ce/Repositories/TRUEOS-Blueprints`

```sh
cargo bp trauma > /tmp/trauma-pack-probe.log 2>&1
```

Exit status: **1**. The host packer built and ran, prepared its private TRUEOS
standard-library sysroot, and then reported:

```text
     Running `target/x86_64-unknown-linux-gnu/debug/trueos-blueprint trauma`
trueos-blueprint: cache miss: no previous success
[standard-library sysroot preparation messages omitted]
error: failed to load manifest for dependency `reqwest`

Caused by:
  failed to read `/home/t4ce/Repositories/trauma/target/trueos-blueprint/work/trauma/TRUEOS-Buildins/vendor/reqwest-0.13.3/Cargo.toml`

Caused by:
  No such file or directory (os error 2)
trueos-blueprint: cargo metadata failed with status exit status: 101
```

The packer's staged manifest resolves the sibling relative vendor path inside
its staging tree instead of the original repository layout. This is the first
observed blocker. The existing vendor directory is present at the intended
original relative path. This blocker was subsequently fixed as described below.

This result does not establish whether middleware versions, reqwest features,
Tokio execution, TLS, progress rendering, filesystem operations, or transfer
sources compile or work on TRUEOS. It exposes an integration blocker and cannot
measure the full porting blast radius. Existing `src/lib.rs`, `src/download.rs`,
and `src/downloader.rs` were left unchanged.

Validation outside packing: the new entrypoint was formatted with rustfmt;
manifest/catalog syntax and declared dependency paths were checked read-only.
No transfer or runtime tests were run during the initial probe. The later
verification pack is recorded below.


## Authorized path fix and verification pack

The packer already rebases dependency paths in
`canonicalize_staged_manifest_paths_from_original` in
`../TRUEOS-Blueprints/src/main.rs`. Its inline-table parser requires the complete
`{ ... }` on one line, while Trauma's original reqwest declaration spanned
several lines for its features array. Thus only reqwest was skipped. All the
other local paths had already been rebased correctly.

`Cargo.toml` now expresses reqwest as a standard `[dependencies.reqwest]` table,
with a separate `path =` field, which the existing staging canonicalizer supports.
Its version, features, and source location are unchanged. There are no packer
changes, machine-specific absolute source paths, or manual generated-stage edits.

The followup ran exactly one verification pack, in
`/home/t4ce/Repositories/TRUEOS-Blueprints`:

```sh
cargo bp trauma > /tmp/trauma-pack-verification.log 2>&1
```

Exit status: **1**. The original missing-manifest error is gone. Read-only
inspection of the regenerated
`target/trueos-blueprint/work/trauma/source-overlay-app/Cargo.toml` verified that
reqwest, tokio, tracing, normal and dev tracing-subscriber, and target-specific
trueos all resolve to absolute existing vendor directories with `Cargo.toml`
files. The checked-in manifest retains portable sibling-relative paths.

The new failure is a dependency feature conflict during Cargo metadata:

```text
    Updating crates.io index
error: failed to select a version for `crossterm`.
    ... required by package `comfy-table v8.0.0`
    ... which satisfies dependency `comfy-table = "=8.0.0"` of package `trauma v3.0.0 (/home/t4ce/Repositories/trauma/target/trueos-blueprint/work/trauma/source-overlay-app)`
versions that meet the requirements `^0.29` are: 0.29.0

package `comfy-table` depends on `crossterm` with feature `windows` but `crossterm` does not have that feature.
help: available features: bracketed-paste, default, derive-more, event-stream, events, filedescriptor, libc, osc52, serde, use-dev-tty


failed to select a version for `crossterm` which could resolve this conflict
trueos-blueprint: cargo metadata failed with status exit status: 101
```

This involves the existing `comfy-table` dev dependency and the TRUEOS crossterm
fork selected by packing. It is separate from manifest path rebasing. No feature
changes, dev dependency removal, source port, or further pack attempts were made.
Trauma still has not compiled, so runtime behavior and the broader porting blast
radius remain unverified.

## Comfy-table feature selection and successful pack

The next followup disabled comfy-table's default features in `Cargo.toml`:

```toml
comfy-table = { version = "=8.0.0", default-features = false }
```

Its default `tty` feature enables optional Crossterm support. The existing
`with-report` example only uses plain `Table`/`Row` rendering, so it does not
need that integration. No vendored fork or compatibility feature was added.

Ran `cargo bp trauma` from `../TRUEOS-Blueprints`, logging to
`/tmp/trauma-pack-comfy-features.log`. Exit status: **0**. Cargo metadata resolved,
Trauma compiled in release mode, the CABI pack guard reported `compatible=1`,
and the packer produced `../TRUEOS-Blueprints/dist/trauma.bp`. The command also
automatically published that artifact to its configured TRUEOS_SITE apps share.

This supersedes the earlier unresolved-build status. No transfer source changes
were necessary for this build. Runtime execution, HTTP behavior, and the resume
correctness findings in `IMPROVEMENT_PLAN.md` remain untested and unchanged.


## Dependency reduction and platform type pass

The subsequent dependency cleanup retains HTTP(S), HTTP/2, body streaming,
request retries, bounded concurrent downloads, and Tokio execution. It changes
presentation and supporting dependencies:

- Removed direct `form_urlencoded`, `reqwest-tracing`, `thiserror`, and
  `tracing-subscriber` dependencies. Error display, source chaining, and
  conversions now use ordinary Rust implementations with the existing variants.
- Replaced the direct Futures facade with narrowly featured `futures-util`.
  Kept middleware and retry logic. Retry tracing is opt-in (`retry-tracing`).
- Made Indicatif and its style APIs opt-in (`terminal-progress`). The default
  application prints summaries without terminal progress rendering. No new
  progress-event API or replacement UI4 renderer is claimed.
- Removed Reqwest charset support (only body bytes are consumed). SOCKS and
  automatic system proxy discovery are opt-in features; explicit HTTP proxy
  configuration remains available. Rustls, HTTP/2, and streaming remain enabled.
- Replaced Tokio's explicit multi-thread-runtime request with `rt`, `fs`,
  `io-util`, `net`, and `time`. Examples use current-thread runtime macros from
  the sole dev dependency. Platform feature unification can enable additional
  capabilities; this does not guarantee the entire graph has no macro crates.
- Removed all former dev libraries: color-eyre, comfy-table, console,
  opentelemetry, rand, and tracing-subscriber. Reports use plain text and the
  resume example requests a deterministic partial length.
- Retained the public Bon builder API, with its runtime crate's `std` feature
  disabled and `alloc` enabled. Tracing's direct request no longer enables
  attribute macros. These are compile-time/feature reductions, not measured
  per-crate binary savings.

Portable traits and heap types explicitly use `core` and `alloc` across owned
sources and examples. Paths go through `trauma::path`, selecting
`trueos::platform::path` on TRUEOS/zkvm and a host fallback elsewhere. The
platform API's `tokio-runtime` feature exposes that facade. Its current types
are standard-library reexports, not an independent no_std path implementation.
OS I/O errors, stdin/stdout, current-directory access, and the host fixture's
sockets/files/threads remain std-backed. No core2/core3 dependency was added.

The default actual-target dependency tree no longer includes Indicatif,
reqwest-tracing, OpenTelemetry, tracing-subscriber, comfy-table, or encoding_rs.
Removing direct dependencies does not remove every transitive instance:
reqwest-retry still requests the Futures facade/executor; retry/TLS dependencies
still request Rand; middleware still uses thiserror. These remain part of the
preserved network/retry stack. A local path pin for the newly selected
futures-util was also evaluated, but its vendor manifest is not independently
resolvable outside the kernel workspace. It remains exactly registry-pinned
rather than changing the kernel workspace for this cleanup.

### Verification and size

- `python3 tools/check_host.py`: passed the four existing unit tests plus two
  loopback/error-contract integration tests, with lean and optional presentation
  configurations, compiled all applicable examples, and passed the doc test.
  The harness compiles the real Trauma library/examples/tests against vendored
  Tokio/Mio and upstream Reqwest 0.13.3 without TLS. This deliberately avoids
  unrelated host-branch failures in the platform Reqwest/Hyper dependencies.
- Actual TRUEOS target: `cargo check --lib --bin trauma --examples --all-features`
  passed using the prepared private std wrapper, JSON target, and complete
  packer source overlays. This covers the real vendored HTTP stack, TLS feature
  selection, optional rendering, retry tracing, and the platform path facade.
  Full log: `/tmp/trauma-trueos-check.log`.
- Default release pack: passed with CABI compatibility, 26 imports, publishing
  disabled through `TRUEOS_BLUEPRINT_SKIP_APPS_PUBLISH=1`. Log:
  `/tmp/trauma-lean-final-pack.log`.
- Earlier artifact: 2,712,195 bytes. Current cleanup artifact: 2,508,338 bytes,
  a reduction of 203,857 bytes (about 7.5%). Both used the packer's release
  profile and LZ4 settings; no size-optimization/LTO tuning was introduced.
- Direct rustfmt checks and `git diff --check` passed.

The loopback fixture drains the Tokio runtime's pending filesystem jobs before
checking saved bytes. It does not prove immediate file visibility at summary
return: explicit flush-on-completion remains part of the deferred correctness
plan. VMX execution, platform network behavior, TLS handshakes, and all resume
correctness changes in `IMPROVEMENT_PLAN.md` remain untested/unimplemented by
this cleanup. The initial downloaded-fully/resume findings have not been fixed.


## ELF layout experiment

Ran `python3 tools/packing_experiment.py` against the existing release artifact.
The experiment does not rebuild Rust sources or change/publish the normal app.
It uses the packer's existing coalescing linker script and toolchain linker,
with relaxation disabled for the extra relink. Local stripping alone uses
`objcopy --discard-all`; the combined variant uses the linker's `--discard-all`.

| Variant | Raw ELF bytes | Packed BP bytes | Section count |
| --- | ---: | ---: | ---: |
| Current pack | 8,946,976 | 2,508,338 | 22,386 |
| Discard unused local symbols | 8,665,720 | 2,429,651 | 22,386 |
| Coalesce sections | 5,825,520 | 1,994,157 | 15 |
| Coalesce + discard local symbols | 5,541,400 | 1,939,684 | 15 |

The compact candidate saves **568,654 bytes (22.67%)** relative to the current
cleaned build. The saving is packaging metadata, rather than fewer original
machine instructions or removed download behavior. Coalescing moves alignment
padding inside the combined sections, so their size must not be interpreted as
an increased instruction count.

Validation against the original artifact:

- The baseline repack matches the original `.bp` byte-for-byte, establishing
  identical archive/header/compression settings for this comparison.
- All 2,992,436 original allocated file bytes match at their new mapped offsets;
  section allocation/write/execute flags, alignment, and BSS mapping agree.
- All 89 undefined imports and 6,317 global definitions (including their mapped
  destinations, types, visibility, and sizes) are preserved.
- All 63,006 allocated-section relocations retain their logical patch locations
  and destinations. GOT references preserve the symbol target and addend
  separately; no references were replaced with unsupported relocation types.
- The `_start` entry point and Blueprint entry hint are mapped to the new layout.
- The LZ4 archive roundtrips to the exact candidate ELF. Negative checks reject
  both an altered code byte and an altered relocation addend.

Candidate: `target/layout-experiment/trauma.compact.bp`.
Machine-readable receipt: `target/layout-experiment/results.json`.
The ordinary `../TRUEOS-Blueprints/dist/trauma.bp` and packer remain unchanged.
This is a statically checked experiment, not a successful VMX execution test.
The raw payload of the compact candidate is 5,541,400 bytes; its packed size is
1,939,684 bytes (about 1.94 MB / 1.85 MiB).


## Maintained as a builtin

The source copy now lives directly at `TRUEOS-Buildins/trauma`, with local
`../vendor` paths. It replaces the retired `hyper` application in `buildins.json`;
the Hyper networking library and its platform overlays remain in use. The old
standalone Blueprint catalog entry has been removed.

The release layout experiment is implemented in the shared buildins driver,
including validation of allocated contents, relocation destinations, imports,
and the entry point. `cargo rigreal` reaches that release packaging path;
`cargo rig` keeps development packaging without the extra link. The shared
buildins workflow owns the artifact and AppDB generation. Earlier standalone
paths and receipts in this document are historical evidence.
