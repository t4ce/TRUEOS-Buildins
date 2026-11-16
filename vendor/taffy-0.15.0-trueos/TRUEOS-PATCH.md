# TRUEOS portability patch

Upstream: https://github.com/DioxusLabs/taffy, published crate 0.15.0 (MIT).

The 64-bit CompactLength backing field stores its packed numeric value and tag
as a usize, rather than as an artificial pointer. Bit encodings, tag masks and
public APIs are unchanged. Actual calc handles still use the original pointer
conversion helpers. Hull enables calc handles; CSS string parsing remains disabled.

This addresses a suspected target-specific source of invalid dimension tags
seen in Blueprint execution. The exact example.com page passes host debug and
release tests; validation on the physical target is still required.

The high-level tree also exposes `compute_layout_with_measure_and_calc`, passing
an invocation-scoped resolver through TaffyView. Existing callers retain their
zero resolver. Hull uses aligned integer registry handles (never dereferenced),
with expressions owned by that layout pass, so percentages resolve against the
containing block provided by the layout algorithm.
