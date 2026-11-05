# Hull source and CDN loading

Production Hull enables host-owned source preparation for classic external
scripts, module scripts and `import()`. It uses the existing netfs download tickets;
there is no network operation inside a QuickJS loader callback.

Bare npm specifiers use **esm.sh first**, then **jsDelivr +esm**:

```js
import('is-number@7.0.0').then(module => console.log(module.default(42)));
```

This requests `https://esm.sh/is-number@7.0.0?target=es2022&bundle`. If source
download or dependency preparation fails, the package root switches to
`https://cdn.jsdelivr.net/npm/is-number@7.0.0/+esm`. Its imports then resolve
against the fallback provider. No failed module body is replayed. JavaScript
runtime exceptions do not switch providers after side effects have occurred.
Pin versions for reproducible results; unversioned/ranged requests can resolve
differently at different providers. Simple explicit esm.sh package URLs also
have fallback; generated build assets, GitHub/JSR paths and custom query options
are not guessed into another CDN's URL layout.

Site-relative and ordinary absolute URLs retain their own origin. A failed
`https://crates.io/_app/...` request is never rewritten into an npm package.
Supported URLs are HTTPS without embedded credentials. The resolver admits
basic import-map exact/prefix entries, including normalized relative URL keys;
scopes and non-JavaScript import attributes fail explicitly.

## Execution and downloads

Rust prepares source graphs, using QuickJS's own parser to discover static
requests. No regex import scanner, JS transpiler or source rewrite is involved.
The synchronous QuickJS module callback reads prepared memory only. Dynamic
imports retain their real Promise resolvers while the host downloads and prepares
the graph. Completion uses QuickJS's normal linker, namespace cache and evaluator,
including cycles, live bindings and top-level await. Module identity includes
fragments. Relative imports and import.meta.url use the selected module URL.

External script downloads are prefetched with a FIFO queue and four concurrent
tickets. Evaluation follows the collected source order. Repeated classic script
elements execute separately, with their own currentScript; repeated external
module URLs share QuickJS module execution. Navigation cancels tickets and drops
the realm, promises, aliases and RAM-only source cache.

Limits per page: 64 source URLs, 4 MiB per source, 8 MiB aggregate, 32 outstanding
dynamic imports, 256 static requests per module, 4 KiB URLs, 2 seconds per fetch,
and 5 seconds cumulative resource waiting (downloads and between-turn graph
preparation). At most four module probes are admitted per turn with a cooperative
20 ms preparation deadline. The original 250 ms cumulative active
JavaScript allowance remains, including parser/host work charged by engine calls.
The initial 250 ms wall window excludes resource waits, with an overall initial
cap of 5.25 seconds. Late imports use remaining page budgets and do not cause a
second render snapshot. No general fetch, XHR, filesystem, timer or process API
is exported. Source/count/wait failures and unhandled rejections are reported.

This is not a full browser fetch policy. The current netfs body-ticket API does
not expose response MIME/CORS/final redirect URL metadata; it uses requested URLs
for resolution and cannot enforce those browser response checks. Canonical CDN
endpoints work without redirect metadata. Parser-time async/defer scheduling,
dynamic script-element insertion, scoped import maps and JSON/CSS module types
are not implemented. Other missing DOM/window APIs still fail normally.

## Offline sandbox

Capture an entry and its static graph without executing downloaded code:

```sh
python3 hull/tools/sandbox/sandbox.py capture https://example.com/ /tmp/page \
  --scripts --module is-number@7.0.0
python3 hull/tools/sandbox/sandbox.py replay /tmp/page --modules --turn-ms 0
```

Repeat `--module URL_OR_PACKAGE` for computed/dynamic entry points. The capture
command uses the native parser/resolver helpers, follows static dependencies,
tries the fallback if a primary graph cannot be captured, and records failures.
Dynamic imports cannot be enumerated without running arbitrary application code;
missing URLs appear in replay resource diagnostics and can be added as seeds.
`--modules` replay reads captured resource tickets only and never opens a socket.
The older `--external` flag only admits already-captured external bodies, without
turning on module graph loading.

`report.json` contains resource events separately from CSS requests. `console.log`
and `trace.jsonl` record requests, successful bodies, fallback and errors. Source
loading itself writes no production assets to disk. Capture files are explicit
development artifacts. Run the sandbox tests and tools/js-tests host suite for
end-to-end checks without building or booting the kernel.


## Remote SVG assets

The native image service converts URL-backed SVG page images and favicon
candidates with `https://wsrv.nl/`, requesting `output=png`, the bounded image-box
width/height, `fit=inside` and one frame. SVG path suffixes use the bridge directly;
extensionless responses can enter it after a bounded SVG document-prefix check.
The full source URL is encoded as one query parameter. The existing PNG decoder,
alpha handling, resize, generation cancellation and UI4 upload own the result.
Regular raster formats keep their native path. Inline SVG rendering and the
page's diagnostic `layout.svg` export are separate from this asset bridge.

Only successfully decoded and published conversions are persisted. The cache is
`<TRUEOS_APP_FS_ROOT>/svg-assets.redb` (normally `apps/hull/svg-assets.redb`), beside
the domain directories. It reuses `trueos-redb::ImageDatabase` with asynchronous
TRUEOSFS reads and complete-image commits. Keys include the canonical source URL,
conversion settings and raster size. Missing/unavailable/corrupt caches are
misses; a write failure does not undo an already published image. No original SVG
or ordinary raster download is written by this cache.

The cache admits 256 entries and 32 MiB of PNG payload, with a 64 MiB database
image limit and the existing 8 MiB per-image bound. Reaching the count/size limit
resets the disposable cache before adding the next conversion. There is no
local expiry/revalidation; deleting `svg-assets.redb` clears it. wsrv must be able
to retrieve the source URL publicly, so local preview URLs and authenticated
assets cannot rely on remote conversion. Provider failures leave the existing
placeholder/fallback behavior in place.

Host checks from the TRUEOS repository, without a kernel build:

```sh
python3 tools/testpy/test_hull_svg_policy.py
python3 tools/testpy/test_hull_svg_cache.py
python3 tools/testpy/test_hull_image_policy.py
```

The regular Hull sandbox does not fetch or paint native image assets.
