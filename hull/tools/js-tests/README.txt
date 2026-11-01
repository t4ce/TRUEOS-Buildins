Host fixtures compile Hull's production script extractor and JavaScript actor,
using a minimal trueos facade over the repository's Tokio and the native
hull-quickjs engine. They do not include a DOM, browser API environment, network
client, the qjs-web-lab copy, or kernel UI services. Input adapter fixtures
use the actual Winit TRUEOS/core record types; they never create a window.

The default is deliberately inactive. Actor startup and page replacement never
execute or classify scripts. Fixtures call Runtime::run explicitly to test the
available execution capability. Navigation drops the old VM and pending work;
the replacement page stays inactive until another explicit run request.

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
No input fixture exposes or connects a DOM, layout, SVG, or renderer.

Input throughput regressions admit at most 32 messages per UI turn and verify a
97-message FIFO burst spans at least four turns, with Promise jobs between
callbacks. Separate fixtures cover two initialization-time queue overflows
without losing accumulated reset counts, a page overriding Object.isFrozen,
and native keyboard records preceding translated modifier-state callbacks.
