# Trauma transfer correctness: implementation record

The three focused patches proposed by the original source review are implemented in this TRUEOS-Buildins source copy. The original review examined checkout `58019f9cddc01de8713d23fd38309dd9da577e8a` on 2026-11-02; its findings were source-derived, without a runtime reproducer at that time. The new loopback regressions now exercise those paths against the current code. This document describes local fork behavior, not the published crate.

## Patch 1 — Validate GET before append or restart (P1, implemented)

`src/transfer.rs` builds a private transfer plan before any destination mutation. `src/downloader.rs` uses HEAD as metadata, checks its status, and falls back to a full GET on HEAD 405/501. Other HEAD failures preserve the destination. Only the `bytes` Accept-Ranges unit is supported.

- GET 200 selects a zero-offset restart, even after a Range request. Existing checkpoint metadata is invalidated before truncation.
- GET 206 requires a requested suffix, a single valid Content-Range with the exact starting offset and full known total, and compatible Content-Length. This implementation expects that one response to reach the representation's end. The returned strong ETag, resolved URL identity, stored total, and current HEAD total must agree.
- Missing/malformed/unsolicited ranges, multipart byte ranges, incompatible Content-Encoding, conflicting validators, and 416 fail before opening the destination. Other GET statuses, including unsolicited 304/204, cannot become success.
- Managed requests use Accept-Encoding identity; automatic Reqwest decompression is explicitly disabled even if compression features are unified by another dependency. Custom Range/If-Range and incompatible Accept-Encoding values fail before sending requests.
- The response-body-length equality skip was removed. A 1,000-byte object with a saved 500-byte prefix and a 500-byte response remainder now reaches Success with exactly 1,000 bytes.

## Patch 2 — Bind prefixes and skips to a representation (P1, implemented)

`src/checkpoint.rs` provides a bounded, versioned `<output>.trauma-resume` sidecar. It stores SHA-256 fingerprints of the requested/resolved URLs, a strong ETag, the known full length, and partial/complete state. It does not persist URL credentials or query strings as plaintext. Weak and absent ETags do not authorize resumable checkpoints. Last-Modified fallback is deliberately unsupported.

- Existing data can resume only with a matching partial checkpoint and successful compatible HEAD metadata; requests carry Range and If-Range. Legacy files, malformed/missing checkpoints, changed identities, partial files already at the total length, oversized local files, and disabled resume all select a full GET.
- A partial checkpoint is atomically published before writing response body bytes. It remains partial on body, write, flush, length-validation, or completion-publication failure. Completion is published only after writes flush and the known full length is satisfied.
- A skip requires an existing regular output file, a completed checkpoint, matching actual/remote lengths, and matching current remote URL identity and strong ETag. Arbitrary equal-length files and partial checkpoints never qualify. Missing empty files are recreated rather than falsely skipped.
- Publication uses a create-new temporary file in the same directory, flush/drop, and atomic rename. `src/checkpoint_rename.rs` uses host Tokio rename and the existing native libc rename import on TRUEOS/zkvm, avoiding TRUEOS Tokio's no-replace behavior and the private std copy/remove implementation. Failed publication removes its temporary file and preserves the previous checkpoint.

The SHA-256 dependency is pinned to the existing 0.10.9 graph version with default features disabled. This hashes resource identity; it does not verify downloaded file content.

## Patch 3 — Count committed bytes and finalize terminal progress (P2, implemented)

`src/body.rs` streams through a private sink interface used by the real Tokio File and injected-failure tests. `src/downloader.rs` reconciles every returned summary with destination metadata and advances the main progress bar at one terminal point.

- Restart begins at zero; validated append begins at the saved offset. Child progress uses the full representation total, with an unset length for unknown totals.
- Bytes/progress advance after successful writes. The sink flushes on stream/write failures as well as EOF before reconciling metadata. A partial write failure reports actual saved bytes rather than the attempted whole chunk.
- Known short/oversized bodies and flush failures cannot report success or publish a complete checkpoint. Unknown-length successful bodies report their resulting actual size.
- Summary size consistently means resulting on-disk length when it can be determined, including skips and failures; zero is used if no length can be established. `Summary::fail` preserves its existing metadata.
- Every returned summary increments the main bar once, including skips and failures. A private guard finalizes all created children on early return; abandoned children retain the saved byte position, including when removed from the display. The renderer and its handles remain gated by terminal-progress.

## Executed regression coverage

`python3 tools/check_host.py` runs the real portable sources with vendored Tokio/Mio and an upstream Reqwest HTTP backend without TLS. It checks lean and terminal-progress/retry-tracing/socks/system-proxy configurations, documentation examples, and the deterministic resume example. Existing vendor Mio unsafe-operation warnings and a Tokio documentation warning remain outside this change.

`tests/resume.rs` uses a scripted loopback HTTP server and temporary directories (`tests/support/mod.rs`). The tests assert literal output bytes, request headers, statuses, and saved lengths. Partial checkpoints are created through an actual interrupted HTTP response before testing resume:

- 1,000/500/500 resume, correct If-Range, exact final bytes, subsequent verified skip;
- ignored Range / If-Range representation change returning 200, without prefix duplication;
- absent, duplicate, malformed, wrong-bound, wrong-span, unknown-total, multipart, changed-ETag, encoded, and 416 partial responses preserving the prefix;
- equal-length legacy files and completed-validator changes requiring full replacement;
- HEAD error lengths never producing a skip, HEAD 405/501 falling back to GET filename inference;
- missing/corrupt/weak checkpoints, missing ETag, oversized or total-length partial files, changed URL identity, and disabled resume selecting full transfer;
- weak/missing ETags succeeding without a checkpoint, unknown-length bodies reporting actual size;
- invalid Accept-Ranges units and managed-header conflicts never bypassing validation;
- a missing empty output being recreated despite a retained complete checkpoint.

Private unit tests additionally inject a real partial write and a flush failure through a wrapped Tokio File. They inspect actual file bytes, reconciled progress, and the still-partial checkpoint. Short and oversized synthetic streams cannot complete a known representation. Terminal tests inspect progress handles without terminal snapshots: full/unknown lengths, abandoned saved positions, cleared failures, and a mixed success/skip/HEAD-failure/GET-failure/interrupted-body batch with one main completion per summary and all created children finalized. Atomic-publication tests exercise replacement, failed publication preserving the old file, and NUL path rejection. The original retry smoke test still verifies middleware request retry behavior.

Host checks do not execute the TRUEOS runtime or validate platform TLS. Actual target compile/pack and ABI evidence are recorded separately in PORT_PROBE.md.

## Boundaries

Use one writer per destination, across Downloader instances and processes, and reserve the checkpoint sidecar name for Trauma. Local edits, adversarial checkpoint tampering, symlink races, concurrent writers, filename containment/collisions, and content checksums are not addressed by this series. Atomic namespace publication is not an fsync-based power-loss guarantee; abandoned create-new temporary files can remain after a process crash.

The queue-snapshot disconnection and error-string retry classification quoted from another downloader were absent here. This series retains direct indicatif handles and RetryTransientMiddleware's typed request policy. Body errors return a failed summary and resumable prefix for a later call; there is no unbounded body retry loop, queue rewrite, public progress-event API, or unrelated dependency upgrade.

## Hull incremental worker follow-up

The transfer fixes are reused by a prepared single-download context with one HTTP pool. Worker startup consumes `download-worker <generation>` before requesting an interactive lease; ordinary launches retain the existing terminal session. The kernel owns queue admission/authentication/retirement and crash requeue. Trauma drains incremental jobs with at most 32 claimed entries, serializes conflicting output names, waits without cancelling a potentially claiming broker read, and reports job states on its console. Outputs use stable sanitized basenames plus normalized-URL hashes under `common/hulldownloads`; reserved checkpoint names cannot become download targets.

Additional tests exercise 81 dynamically arriving jobs, concurrency bounds, duplicate destinations, broker I/O cancellation safety, pre-binding waits, invalid-payload acknowledgment, acknowledgment failure propagation, launch/frame validation, unsafe filename hints, stable URL-fragment normalization, and actual HTTP interrupted/resume/skip behavior through one prepared context.
