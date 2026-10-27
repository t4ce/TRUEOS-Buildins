# Paper

A standalone editable DIN A4 sheet with gridpaper, linepaper, and whitepaper modes.
The Blueprint owns its UI4 immutable frame through `trueos::ui4_scene`.
It does not use the Paperr service or its snapshot/font API.

The page is 210 × 297 mm. As in Paperr, the centered grid has 39 × 55
cells (195 × 275 mm), leaving 7.5 mm side margins and 11 mm top/bottom
margins. UI4's physical-extent query uses the active output's EDID metrics;
if those metrics are unavailable, the app exits rather than guessing DPI.
Pixel-center snapping limits physical spacing error to pixel quantization.

One V2 indexed batch draws 96 independent native `LINE_LIST` primitives
(192 vertices/indices), single sampled, without adjacency or triangle
expansion. The native line-only batch path first fills the target white
through BCS0 and waits for completion before the RCS line pass. There is no
fullscreen clear triangle. An unavailable BCS engine returns Busy; an
uncertain submitted fill quarantines the target through the existing vGPU
failure path.

The completed frame stays resident between edits and 500 ms caret updates.
Resize preserves physical cell size, clipping or exposing white space
instead of stretching the grid. Closing the window releases the frame and
GPU device. This app does not advertise the replicatable lifecycle.

Build the relocatable object with ordinary Cargo from this app's directory:

```sh
cd paper
cargo build
```

Output: `target/trueos-object/debug/paper.o`. The local Cargo config selects the
TRUEOS target and builds `core`/`alloc`; the repository pins the same nightly as
the kernel. The target's normal linker step uses `-r` and retains `_start`, so
the object includes the selected Rust dependencies. Kernel C ABI imports remain
unresolved for the Blueprint loader. Run from `paper/` so Cargo reads its config.

This uses the default unoptimized dev profile. Strip debug information separately:

```sh
objcopy --strip-debug target/trueos-object/debug/paper.o \
    target/trueos-object/debug/paper.stripped.o
```

LLD ignores stripping during `-r` links, including Cargo's `strip` profile option.
Keep symbols and relocations: the loader needs them. Cargo tracks the original
`paper.o`, so a repeated `cargo build` stays fresh. `cargo check` also works.
This first object workflow covers Paper's `no_std` dependency graph; apps using
`std` still need their TRUEOS standard-library setup.

Requires a kernel built with the additive
`trueos_cabi_ui4_scene_physical_extent_v1` API and BCS0 native-line clear path.

Paper is registered in `buildins.json` and embedded in the kernel AppDB by
normal builds with built-ins enabled; it does not need an online catalog entry.

The one-shot `vFile:launch` runscript is read before opening the UI4 frame:

```text
mode linepaper
step 3x
```

`mode gridpaper` (default) draws a grid; `mode linepaper` draws only horizontal
ruling; `mode whitepaper` leaves the sheet visually blank. These are top-level
launch settings, independent of spacing.

`step 1` through `step 8` select 5–40 mm spacing; `scale` remains an alias.
In lined and white modes, step changes only row height: character columns stay
5 mm wide and glyphs retain their 1x size. Whitepaper retains the same editing
layout without visible ruling. The native line batch uses white lines to preserve
its existing white clear path.

In grid mode, `scale 1` through `scale 8` (also `1x` through `8x`) select 5–40 mm cell
spacing. An absent/empty script defaults to 1x. Invalid directives fail before
opening a window. The A4 frame stays the same size; each scale centers the
largest whole-cell grid within the original 195 × 275 mm grid area. Resize
keeps the selected spacing. Lines remain one pixel wide.


## Editing

Grid mode allocates `(39 / scale) * (55 / scale)` character slots up front.
Lined and white modes allocate `39 * (55 / scale)` slots.
Typing replaces the selected cell and advances, wrapping to the next row;
the cursor stops at the final cell. Space only advances. Arrow keys navigate,
Backspace moves left and clears that cell, Delete clears in place, and Enter
currently does nothing. Click a cell to select it; the caret blinks every
500 ms. The document is in memory only. Each cell reserves a foreground-colour
field for future colour editing; current text is black on white.

Glyphs are native Microfont raster-line glyphs, centred by their em box;
unsupported characters display `?`. Paper reads the bounded virtual file
`vFile:microfont-grid/SCALE/HEX_UNICODE_SCALAR` from the shared Shell3 font
service, then uploads one replaceable sprite per occupied cell. Payload is
little-endian `u32 width, u32 height` followed by opaque RGBA8. This requires
the updated kernel. The hexadecimal scalar identifies the character; glyph
pixels come from native raster-line geometry, never hexadecimal bitmap fonts.

At 1x the Shell atlas is reused. Full native R8 atlases are shared at 2–4x;
5–8x request only individual glyphs. Atlas memory is reported at log-os `ONCE`
level once per full-atlas em height, including GPU backing and retained CPU
mask bytes (shared geometry excluded). Text and caret are BCS0 copies/fills
over the existing native line pass. Sprite slots are bounded by page capacity.
