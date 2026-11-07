# TRUEOS portability

Vendored from the locally cached crates.io package xml-1.4.0. Original licenses and parser sources are retained.

The `trueos-native` feature builds the existing lexer, parser and writer with core/alloc and shared `trueos-io` traits. String maps and deduplication sets use BTreeMap/BTreeSet; default builds retain std I/O and HashMap/HashSet. Entity expansion limits, namespaces, encodings, recovery options and all syntax machinery remain upstream. Native I/O error clones preserve kind and static detail without heap leaks. An invalid UTF-16 surrogate remains InvalidData; its native static diagnostic omits the numeric surrogate value because shared I/O errors accept static details. Upstream std-I/O unit modules run on the default configuration; the shared native boundary fixture exercises the actual native traits.

Reproducible shared native-I/O tests: `python3 tools/testpy/test_share_xml_native.py` in TRUEOS.
