# TRUEOS portability

Vendored from the locally cached crates.io package xml-rs-1.0.0. Original licenses and parser sources are retained.

The `trueos-native` feature forwards to the existing `xml` dependency and disables the facade's implicit std dependency. This remains a reexport of the same parser, not a second implementation.

Reproducible shared native-I/O tests: `python3 tools/testpy/test_share_xml_native.py` in TRUEOS.
