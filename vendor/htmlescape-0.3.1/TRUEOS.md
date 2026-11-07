# TRUEOS portability

Vendored from the locally cached crates.io package htmlescape-0.3.1. Original licenses and parser sources are retained.

The `trueos-native` feature selects core/alloc and shared `trueos-io`; defaults keep std I/O. The complete upstream entity tables and streaming encode/decode implementation remain. The hex entity prefix now uses write_all instead of write so partial writers cannot truncate it; the boundary fixture covers one-byte and Interrupted writes. Unused test-only num was removed; tests use the cached compatible rand 0.4 API rather than the obsolete 0.3 facade.

Reproducible shared native-I/O tests: `python3 tools/testpy/test_share_xml_native.py` in TRUEOS.
