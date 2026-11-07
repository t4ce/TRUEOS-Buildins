# TRUEOS portability

Vendored from the locally cached crates.io package xmltree-0.12.0. Original licenses and parser sources are retained.

The `trueos-native` feature selects core/alloc, shared `trueos-io`, native XML and the existing `attribute-sorted` representation. Default host builds retain their original std I/O and attribute feature behavior. Namespace, text, comments, CDATA, processing instructions, parser options and writer options remain upstream.

Reproducible shared native-I/O tests: `python3 tools/testpy/test_share_xml_native.py` in TRUEOS.
