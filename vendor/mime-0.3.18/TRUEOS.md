# TRUEOS portability

Vendored from the locally cached crates.io package mime-0.3.18. Original licenses and parser sources are retained.

The library builds with core/alloc on all targets. Parsing, constants, quoted parameters, comparison, formatting and hashing retain the upstream implementation. The empty `trueos-native` feature makes selection explicit for native consumers. Obsolete AsciiExt imports were removed in favor of existing inherent ASCII methods; default host API remains the same.

Reproducible shared native-I/O tests: `python3 tools/testpy/test_share_xml_native.py` in TRUEOS.
