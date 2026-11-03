# HTML project preview

`html-preview` is a standalone Axum Blueprint, included in `buildins.json`.
Edit owns its child VM; HTTP serving runs outside the editor and Codex VMs.

In Edit, choose **IDE → New Project → Create HTML**. This creates `index.html`,
`main.js`, and `style.css` in the normal revisioned project store. The HTML5
starter has an empty body, relative CSS/JS includes, a body background, and one
`template console log` message on `DOMContentLoaded` per page load.

**Preview** saves open project buffers, publishes the project files into an
owned workspace folder, starts the preview child, waits for its bound loopback
address, and opens Hull. Further Save operations update that folder. Press
**F5 in Hull** to reload the page, CSS, JS and images. Opening Preview again
opens another ordinary Hull window on the same server. Each workspace has its
own server port. Closing or switching the Edit project stops its server child;
the browser keeps its normal independent lifetime.

The existing Codex `/ide` integration reads and edits HTML project files.
`trueos_ide` action `open` accepts `kind: "html"`; action `run` saves and opens
preview. Workspace context reports `kind: "html"`, no compiler, and the preview
URL when ready. Native Odin/Rust compiler projects keep their existing path.

## Standalone startup

The normal Blueprint startup stream selects a public folder and a separate
readiness file, using existing TRUEOSFS access:

```text
fs-scope trueosfs
preview-root /apps/common/my-page/public
preview-ready /apps/common/my-page/ready.json
preview-entry index.html
```

The folder must contain the entry file and any relatively referenced assets.
The server binds `127.0.0.1` on an available port and writes a
`trueos-html-preview/v1` readiness record after binding. It serves GET/HEAD only,
sets correct asset MIME types and `Cache-Control: no-store`, rejects parent-path
escapes and host symlink escapes, and caps files at 8 MiB. It has no directory
listing, file mutation endpoint, live DOM bridge, or build toolchain.

Edit owns source publication; complete individual files are replaced on Save.
This is a refresh-based folder server, not a multi-request transactional snapshot
of a page while a save is in progress. Readiness and publication bookkeeping stay
outside the public folder.

On a development host, the same binary accepts a startup-script filename:

```sh
cargo run --manifest-path html-preview/Cargo.toml -- /absolute/path/preview.startup
cargo test --manifest-path html-preview/Cargo.toml
```

Normal `cargo rig` includes the new builtin with Edit and Hull. Repack Codex
with `cargo bp codex-cli` to include its updated HTML project tool/context support.

## Verification

The implementation was checked with the complete 13-app TRUEOS builtin build
and ABI validation, `cargo check --bin TRUEOS --locked`, Edit's IDE binary tests,
the shared source-store/protocol tests, Hull's JavaScript tests (including the
actual starter logging once on each full load), and the standalone Axum process
integration test. The latter serves the real three-file starter, checks MIME
types and HEAD, replaces CSS in the live folder, and fetches it from the same
running process. No rig deployment or reboot was performed for this change.

Useful host checks, from outside the kernel's Cargo configuration directory:

```sh
cargo test --manifest-path /path/to/TRUEOS-Buildins/html-preview/Cargo.toml
cargo test --manifest-path /path/to/TRUEOS-Blueprints/crates/trueos-ide/Cargo.toml
cargo test --manifest-path /path/to/TRUEOS-Buildins/edit/Cargo.toml -p edit --bin edit --features ide
cargo test --manifest-path /path/to/TRUEOS-Buildins/hull/tools/js-tests/Cargo.toml
python3 /path/to/TRUEOS/tools/testpy/test_codex_ide_client.py
```

Broader checkout limitations observed during verification:

- A complete Codex workspace check stops at its existing missing
  `TRUEOS-Blueprints/apps/qjs/crates/trueos-qjs/Cargo.toml` dependency. Its actual
  changed IDE client module passes the separate transport tests; full CLI/TUI
  rebuilding remains to be verified after that dependency is restored.
- Edit's full library suite has two zero-width search failures; its IDE binary
  suite passes. The unchanged Hull script-extraction implementation also fails
  an existing layout fixture that expects repeated classic scripts to be
  deduplicated, while the current loader deliberately retains separate script
  executions. Other layout tests pass. These unrelated behaviors were left alone.
