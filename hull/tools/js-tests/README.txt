Host fixtures compile Hull's production script extractor and JavaScript actor,
using a minimal trueos facade over the repository's Tokio and the native
hull-quickjs engine, minimal DOM gateway and shared selector matcher. They do not
include a network client, the qjs-web-lab copy, or kernel UI services. Input adapter fixtures
use the actual Winit TRUEOS/core record types; they never create a window.

Actor startup and replace_page remain inactive. begin_page admits the parsed
document's inline scripts with one 250 ms active execution allowance and a 250 ms
initial wall window. Initialization fixtures check real rp5 mutations, frozen
render snapshots, faults, navigation reset, infinite sources and microtask
chains. Other fixtures call Runtime::run explicitly to isolate execution paths.

Fixtures cover explicit script/Promise ordering, per-poll work admission,
navigation isolation, native syntax errors, metadata-only JSON/data skips,
external-source metadata without downloads, missing host capabilities, and a
bounded infinite-script interrupt followed by continued engine use.

Run outside the repository's Blueprint Cargo configuration:
  cd /tmp
  CARGO_TARGET_DIR=/home/t4ce/Repositories/TRUEOS/tgt/hull-js-tests cargo test --manifest-path /home/t4ce/Repositories/TRUEOS-Buildins/hull/tools/js-tests/Cargo.toml

Input fixtures exercise production Winit-to-JSON adaptation and the actor's
bounded data queue. They cover native pointer identities/button chords, Unicode
keyboard data/modifiers/raw bytes, loss/reset ordering, IME byte ranges, no
duplicate standard pointer delivery, immutable payloads/snapshots, callback
ordering and Promise jobs, listener errors/once/removal, inactive input discard,
navigation epoch isolation, oversized-input rejection, and queue overflow reset.
Input-only fixtures use inert staging without a DOM. Initialization fixtures
connect the actual minimal DOM, but never create a window or trigger a redraw.

Console fixtures cover explicit pasted execution without requesting collected
scripts, shared page globals, Promise ordering, callback logs, navigation reset,
paste queue/byte limits, console overflow and error recovery. Base64 fixtures
check byte-string round trips, whitespace/padding, coercion and invalid input.
Native engine integration tests additionally cover bounded UTF-8 console text,
basic formatting/assertions, interrupted conversions and monotonic time reads.
Native host/budget fixtures check cumulative time across sources/jobs/callbacks,
idle exclusion, bounded JSON transport, host errors and exhausted-budget refusal.

Input throughput regressions admit at most 32 messages per UI turn and verify a
97-message FIFO burst spans at least four turns, with Promise jobs between
callbacks. Separate fixtures cover two initialization-time queue overflows
without losing accumulated reset counts, a page overriding Object.isFrozen,
and native keyboard records preceding translated modifier-state callbacks.
