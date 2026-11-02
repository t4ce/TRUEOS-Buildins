# Trauma: focused download correctness plan

Reviewed local checkout `58019f9cddc01de8713d23fd38309dd9da577e8a` on 2026-11-02. This is a source review of this fork, not a claim about the published crate. All findings below are source-derived; no HTTP reproducer, build, or tests were run. Dependency restructuring is happening separately. This document proposes three patches; it does not implement them or change dependencies.

## Findings and applicability

| Priority | Finding | Local evidence |
| --- | --- | --- |
| P1 | A server ignoring `Range` can cause a full representation to be appended to an existing prefix. | `src/downloader.rs:180–204` decides append from HEAD metadata before GET. `:219–223` only rejects HTTP errors; `:265–270` appends even when GET returned `200`. `src/download.rs:37–50` checks only advertised ranges and a nonzero length. |
| P1 | An existing prefix is not bound to a representation, and returned ranges are not validated. Same-length remote changes can be skipped or mixed with old bytes. | `src/downloader.rs:189–195` skips on HEAD length alone; `:199–209` sends `Range` without `If-Range`; no ETag, validator checkpoint, or Content-Range parsing exists in `src/`. `src/lib.rs:65–70` treats every Accept-Ranges value except `none` as support. |
| P1 | The remaining response length can be mistaken for the complete file length. | With a 1,000-byte representation and a 500-byte local prefix, HEAD length 1,000 avoids the first skip. A valid GET `206` with `Content-Length: 500` then satisfies `src/downloader.rs:233–237`, returning `Skipped` with only 500 bytes on disk. This is a traced source path, not an executed reproduction. |
| P2 | Byte counts and progress are wrong on restart/resume and incomplete on terminal paths. | `src/downloader.rs:248–249` uses response-body length as the total but local length as position. `:285` starts the final count at the old local size even when `:269` truncates. `:301–314` counts bytes before successful writes. Only the success path finishes the child bar and advances the main bar (`:317–325`); skips and failures return earlier. |

The quoted queue-snapshot disconnection and error-string retry classification from another downloader do **not** describe this implementation. Here, tasks receive shared `Arc<ProgressBar>` handles (`src/downloader.rs:119–121, 137–142`), requests use `RetryTransientMiddleware` (`:83–101`), and body-stream errors return a failed summary (`:289–299`). Error strings are produced for reporting; there is no local string-based retry decision or queue snapshot implementation. Do not transplant fixes for those absent structures. The actual progress defects above still warrant repair. Request middleware also does not amount to a visible application-level retry loop around body streaming in this source.

## Patch 1 — Validate the actual GET before choosing append or restart (P1)

**Scope:** Small private response-planning and Content-Range parsing helpers in `src/downloader.rs` / `src/lib.rs`, plus the existing `fetch` path. Keep the public `Download`, `Downloader`, and `Summary` APIs intact.

- Treat HEAD as metadata/advice. Check its status; never derive a skip or append decision from an HTTP error response. For HEAD `405`/`501`, allow an ordinary GET with filename inference from its successful response or the URL/override. Other failures must have an explicit failure policy and leave existing data intact.
- Require the `bytes` range unit when interpreting Accept-Ranges. A positive HEAD hint permits an attempt, not an append operation.
- Build an internal transfer plan after inspecting GET: `Restart` for `200`, `Append { offset, total, expected_body_bytes }` only for validated `206`. Open/truncate/append the destination only after that plan exists.
- For append, require one satisfiable byte Content-Range starting exactly at the requested local offset; validate end/start/total bounds and any Content-Length against the returned span. Reject malformed/missing ranges, unexpected multipart bodies, and unsolicited partial responses to a full GET. Use a separate known/unknown full length rather than treating missing length as zero.
- A `200` after a range request means restart from byte zero with truncation and zero initial progress. A `416` must not become success or `Skipped` merely because its status or a length looks plausible; preserve the file and fail unless later identity/completion checks prove it complete.
- Remove the GET-body-length equality skip (`src/downloader.rs:232–238`). Patch 2 replaces the HEAD-only skip with representation-aware logic. Prevent caller-supplied `Range`/`If-Range` headers from silently overriding internally managed resume headers; reject conflicting input with a clear summary.

**Acceptance / local HTTP regression cases:** Use a loopback fixture and isolated temporary directories; disable request retries for deterministic protocol cases. Assert request headers, exact output bytes, summary status, and summary size.

1. HEAD advertises 1,000 bytes, local prefix is 500, GET returns `206`, `Content-Range: bytes 500-999/1000`, `Content-Length: 500`: download finishes with the exact 1,000-byte fixture and `Success`, never `Skipped`.
2. Same setup but GET ignores Range and returns the full 1,000 bytes with `200`: output is exactly the full representation, never 1,500 bytes.
3. Missing/malformed Content-Range, wrong start, impossible bounds, wrong span length, multipart `206`, and `416`: fail without changing the existing prefix. Test invalid and unsupported Accept-Ranges values separately.
4. HEAD `405`/`501` followed by valid `200`: full download succeeds. HEAD error content-length matching an existing file must never produce a false skip. A conflicting custom Range header must not bypass validation.

**Boundary:** This patch establishes wire-response correctness. It cannot establish that an old prefix belongs to the current remote object; ship it with patch 2 before claiming safe resume of existing files.

## Patch 2 — Bind saved prefixes and skip decisions to a representation (P1)

**Scope:** A private versioned resume checkpoint beside the output and validator handling in `fetch`; no new scheduling system, public builder redesign, or checksum API.

- Record a strong ETag from the successful GET, a fingerprint of the requested/final URL identity, and the known full representation length when available. Do not store URL credentials or tokens in plaintext. A HEAD ETag describes the current remote object; it is not evidence for which object created a pre-existing local prefix.
- Persist the checkpoint before storing resumable body bytes, using atomic replacement; invalidate old metadata before a restart. Distinguish partial and successfully completed states. Mark completion only after successful writes/flush and expected-length validation. Keep the completed checkpoint if future identity-aware skips are supported.
- Resume an existing prefix only when its checkpoint matches the resource and provides a strong validator; send `Range` plus `If-Range` with that validator. Refuse weak ETags for this policy. If no usable checkpoint/validator exists, download from zero. Last-Modified support can follow later; do not add a weak fallback in this patch.
- Require GET `206` to agree with the stored validator and any known representation length as well as patch 1's range checks. A changed representation returned as `200` follows the restart path and replaces the checkpoint. A conflicting `206` fails before touching the prefix.
- Replace size-only early skipping with a completed checkpoint plus a successful remote identity check and matching full length. An arbitrary equal-length file or partial checkpoint must never qualify. A conservative re-download when validation is unavailable is acceptable. Keep `resumable(false)` as a full restart policy for an existing file.
- Maintain one byte representation across requests and saved offsets: use identity encoding for managed range transfers and reject incompatible Content-Encoding. Verify this with the active reqwest feature configuration rather than assuming decoded and wire-byte offsets coincide.

**Acceptance / local HTTP regression cases:** Extend the same fixture; create checkpoints through an initial interrupted download rather than hand-crafting only the happy path.

1. Initial transfer of ETag `"v1"` interrupts after a prefix; next call sends `If-Range: "v1"` and the correct Range, validates `206`, and produces exact bytes.
2. Replace the remote file with a different representation of the same length (`"v2"`). A valid `200` fallback replaces old bytes; an invalid mixed-version `206` fails with the prefix intact. Equal length must not trigger a skip.
3. Legacy partial file, missing/malformed checkpoint, weak/missing ETag, changed URL identity, and local length greater than recorded total: restart safely rather than append. A missing/invalid checkpoint must not discard unrelated files.
4. Completed file with matching checkpoint and remote validator may skip with the correct final size; changed validator forces a full transfer. Crash/incomplete checkpoint states never qualify as complete. Test an encoded response so byte-range offsets cannot silently refer to a different representation.

**Boundary:** This adds representation consistency, not cryptographic content verification or concurrent-writer protection. Preserve filenames and the existing return types. Document the checkpoint lifecycle and the expected single writer per destination.

## Patch 3 — Count committed bytes and finish every terminal progress path (P2)

**Scope:** Local accounting and progress cleanup around `fetch` and the collection in `download_inner`. Retain indicatif and request retry middleware.

- Track `initial_committed_bytes`, `bytes_written`, and `expected_final_length` separately. Initialize at the validated append offset or zero for restart; derive progress total from the full representation length, never the remainder length. For unknown totals, use indeterminate progress.
- Increment byte accounting/progress only after a successful write. On an I/O failure, reconcile the reported committed size with file metadata where possible because `write_all` may have written a partial chunk before returning an error. A restart over a 500-byte file followed by a 1,000-byte body must report 1,000 bytes, not 1,500.
- At EOF, reject a known short/oversized transfer and flush successfully before reporting success or marking the checkpoint complete. Report `Summary::size()` consistently as the resulting on-disk length, including skips and failures where that length can be determined; document the meaning.
- Advance the main bar once for each returned summary, including skips and failures. Move that operation to one collection/finalization point so early returns cannot miss it or double-count it. Finish/clear or visibly fail an existing child bar on every terminal return using a small cleanup helper/guard.
- Keep body interruption as an explicit failed summary with a valid partial checkpoint for the next call. Do not add string-based retry classification or an unbounded body-retry loop. The existing request retry budget remains separate; body retries, if later desired, need their own bounded policy and validated reopen path.

**Acceptance / local HTTP regression cases:**

1. Valid resumed transfer reports total/terminal position 1,000 and `Summary::size() == 1000`; forced full restart over an existing prefix reports the same final size. A truly completed skip reports the actual complete file size.
2. Loopback server drops a fixed-length body early: status is failure, reported size matches the saved prefix, checkpoint stays incomplete, and the next invocation can resume under patches 1–2. Unknown-length successful bodies report their actual resulting size; known inconsistent totals cannot become success.
3. A mixed batch containing success, verified skip, HEAD/GET failure, and interrupted body ends with one main completion per returned summary and no unfinished child bars. Inspect progress state through a private test seam/hidden draw target rather than snapshotting terminal escape sequences.
4. Inject a write/flush failure through a narrow test seam: no success/completed checkpoint, and reported byte counts do not include an unwritten whole chunk. This requires a meaningful failure fixture, not a test that merely repeats arithmetic from the implementation.

## Delivery and verification

Implement in the order above, with protocol and validator fixes reviewed together. Add deterministic loopback HTTP tests to the existing repository rather than external download examples; cover literal byte contents as well as lengths to expose mixed representations. Keep test-only dependency/feature choices aligned with the separately updated manifest. Once dependencies settle, run focused regression tests, existing unit/doc tests, formatting, and the repository's required lint/check commands. No runtime validation is claimed by this planning document.

Do not expand this work into a queue rewrite, a new public progress/event API, retry-string parsing, or unrelated dependency upgrades. Filename containment and collisions are separate review topics: the current `directory.join(filename)` path (`src/downloader.rs:155–164`) merits follow-up, but is outside these three transfer-correctness patches.
