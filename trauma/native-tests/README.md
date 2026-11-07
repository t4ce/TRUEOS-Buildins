These host fixtures execute Trauma's actual native HTTP and buffered-storage
adapters against deterministic native service responses. The build script selects
the native branches of the actual body/checkpoint modules and excludes their
separate hosted tests. It does not copy their transfer implementation.

Run from the Trauma checkout after these fixtures are installed as `native-tests`:

```sh
cargo test --manifest-path native-tests/Cargo.toml --target x86_64-unknown-linux-gnu --target-dir ../../TRUEOS/tgt/trauma-native-fixtures
```

The cases cover durable byte accounting, checked publication and races, protected
typed reset, checkpoint ordering, known-length body errors, redirects and secret
headers, transient versus fatal retries, admission pressure, and cancellation.
The mocked services do not substitute for kernel storage/transport tests or the
actual `core`/`alloc` target build. Those checks and rig verification are recorded
in the TRUEOS build receipt.
