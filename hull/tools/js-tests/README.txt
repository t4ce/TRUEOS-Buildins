Host fixtures compile Hull's production script extractor and JavaScript actor,
using a minimal trueos facade over the repository's Tokio and the native
hull-quickjs engine. They do not include a DOM, browser API environment, network
client, the qjs-web-lab copy, or kernel UI services.

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
