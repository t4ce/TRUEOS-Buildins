# edit / shader studio — text as input

A buildless browser studio in which the actual shapes of editor text drive eight GLSL effects. Aurora is the default. This is a browser implementation of the glyph-mask/compositing idea; the linked native TRUEOS repositories are not modified.

## The input path

1. Commit the text change immediately to the HTML editor.
2. On the selected cadence, render the visible text into a separate transparent Canvas2D surface using its font, line height, letter spacing, gutter offset, wrapping, and scroll position.
3. Extract the **alpha byte** of every pixel into a `Uint8Array(width * height)`. Background pixels are zero; antialiased glyph edges retain partial coverage. RGB, syntax color, selection, caret, menus, status bars, and shader output are excluded.
4. Queue this immutable image. The renderer uploads the newest image in its own scheduled frame, using **WebGL 2 `R8 / RED / UNSIGNED_BYTE`**. WebGL 1 falls back to a one-channel `LUMINANCE` texture. Both are sampled through `.r`.
5. Feed the retained mask into a persistent, double-buffered GPU field. The previous field is read while the next is written. Glyphs emit continuously; new snapshot revisions also inject a pulse.
6. Sample the raw text and evolving field in the selected effect. Composite only the document background at the chosen opacity.

This is glyph coverage, not brightness and not a character-count parameter. Extracting alpha from an already opaque page screenshot would yield an all-white mask, so capture happens in a transparent text-only pass before background composition.

## Cadence and controls

- **Each key:** input events request a snapshot outside the input handler. Multiple events within one animation frame coalesce to the newest text.
- **3 seconds:** captures every 3000 ms while the document is visible, even if the text is unchanged. The new snapshot pulses the feedback effects; Life emits only when there is new glyph input.
- **Manual:** keeps the captured image unchanged until **Snapshot now**. The shader continues evolving with that image.
- **Snapshot now:** explicitly captures immediately in any mode.
- **Inspect glyph coverage:** shows the exact captured R8 image, dimensions, revision, age, byte count, and coverage. Raw `.r8` download uses top-down rows, one byte per pixel; dimensions are in its filename.
- **Text influence:** defaults to 100%. An empty glyph mask and cleared feedback reduce the seven feedback effects to their dark base. Existing Life cells keep evolving after text is cleared; a manual restart with an empty capture clears that world.
- **Trail persistence:** controls decay of the GPU field.
- **Clear trails:** resets the feedback surfaces while retaining the current input mask.
- **Pause:** stops simulation time, while editing and new input uploads remain available.

Scroll, resize, wrap, and viewport changes mark the image dirty. Each-key mode updates the aligned snapshot immediately; three-second mode retains the last image until its next tick, even if typing scrolls the viewport. Manual mode retains it until an explicit capture. Capture and animation stop scheduling work while the browser page is hidden.

## Editor typography

**View → Text size** provides − / + controls from 10 to 48 px in 2 px steps, a current-size readout, and **Reset text size** (14 px). The menu stays open for repeated adjustments. Only the editable text and its aligned line layout change; menus, toolbars, status text, and inspector keep their interface typography.

**View → Block font** switches the document and shader-source editor to **Edit Pixel Mono**, a compact WOFF2 adaptation of the supplied Pixel UniCode font. It includes A–Z, a–z, 0–9, all printable ASCII punctuation (including JavaScript brackets, braces, operators, quotes, backticks, and escapes), and non-breaking space. Other characters use the existing fallback font. Pixel outlines are retained, with equal-width character cells for code alignment. Use 16 or 32 px for whole-pixel glyph scaling; block mode uses zero letter spacing and an even line box at those sizes.

The textarea, syntax layer, and glyph capture share the selected font and size. Font loading triggers a layout refresh and invalidates the R8 input after the selected face is ready. Each-key mode captures then; three-second and Manual modes retain their existing cadence. Both settings persist in the browser and studio setup export/import, and resetting shader appearance keeps them.

The 2,672-byte derivative is based on **Pixel UniCode by ivancr72**, licensed under **CC BY 3.0**. The original license/readme and adaptation credits are bundled in `dist/fonts/`; credits are also accessible from **Help → About shader studio**. The uploaded archive is unchanged.

## Effects

| Effect | How it consumes text |
| --- | --- |
| Aurora | Pulls glyph coverage and the diffusing field into moving light curtains. |
| Wave | Samples outward offsets from letter silhouettes, with snapshot-triggered phase resets. |
| Cubes | Glyph occupancy and field density control the visibility and height of isometric blocks. |
| Contours | Draws level sets of the diffusing text field. |
| Dither | Quantizes drifting glyph coverage and field energy into ordered pixels. |
| Echo | Carries previous letter shapes forward as luminous trails. |
| Game of Life | Adds new glyph coverage to a persistent Conway B3/S23 grid while existing cells keep evolving. |
| Matrix rain | Text-derived energy controls green falling glyph streams with bright heads and fading tails. |

The preset previews receive the same captured document mask. They are paused samples of each effect, not decorative images.

## Game of Life

The first captured text supplies the initial cells. Later captures **emit into the existing world**, without resetting its generation counter or clock. A separate GPU pass adds new glyph coverage to the current cells; it never erases the world. Between emissions the simulation follows Conway’s B3/S23 rule: a dead cell is born with exactly three live neighbors; a live cell survives with two or three. The eight surrounding cells are its neighborhood. Separate nearest-filtered GPU surfaces preserve binary alive/dead state.

The world has finite dead boundaries and approximately one cell per three CSS pixels, capped at 512 × 512 cells. It advances ten generations per simulation second. Changing render quality preserves the world. A viewport resize copies overlapping cells into the resized grid, anchored at the top left; new space starts empty. Fading trails and birth highlights are decorative channels and never alter the rules.

### Syntax colors and mixing

New cells inherit the rendered color of the glyphs that emit them. The color source is the existing syntax-highlighted text, read from its computed styles; keywords, strings, numbers, comments, headings, and plain text keep their actual editor colors. With syntax highlighting disabled, new cells use the editor’s plain text color. Life’s color hint replaces the palette selector; the other seven effects keep their palette controls.

Surviving cells retain their color. A newborn with three same-color parents inherits that color unchanged. If its parents have different colors, their RGB bytes are added channel by channel modulo 256. Incoming glyph ink landing on an already living cell uses the same wraparound addition with the cell’s current color. For example, a channel value of 200 plus 100 becomes 44, and 255 plus 1 becomes 0. Dead-cell trails retain their last color while fading.

Color is stored separately from occupancy, trail strength, and birth flags. Even a black cell remains a live cell under the normal rules. Color mixing never changes B3/S23 behavior, and it does not cause a restart.

**Each key** emits new ink after a captured edit. **3 seconds** batches those changes until the next tick. **Manual** waits for **Snapshot now**. Spaces, tabs, newlines, and repeated unchanged snapshots do not emit. Deleting text never kills cells already living in the world. The current raw glyph image still updates for the inspector and the other effects.

**Scrolling is the refresh exception:** a changed captured horizontal or vertical scroll position replaces the Life world with the newly visible glyphs and resets its generation counter. The renderer’s animation clock continues. This follows the selected capture cadence and also applies when typing or moving the caret scrolls the editor. Edits at unchanged scroll offsets retain the additive behavior, including whitespace-only edits.

**Restart life / Restart from captured text** explicitly replaces the world with the retained full glyph image and resets its generation counter. The existing **Restart animation** button also restarts the renderer and clock. Preset selection, shader compilation, font changes, and snapshots at unchanged scroll positions do not restart Life. Pause freezes generations while accepting new input; speed scales the generation clock. Reloading the page or recovering from GPU context loss necessarily creates a new world from the retained input.

### Capture-to-Life contract

Each snapshot has an ordinary `revision` and a separate `emissionRevision`. The former advances on every capture. The latter advances only when the captured text with whitespace removed, or its selected typography, changes. Font readiness is included so a newly loaded web font can emit its actual glyphs after a fallback-font capture. This comparison uses the actual captured content, so paste, undo, replacements, and composed input use the same path as typing. No source text is passed to the renderer.

The R8 `data` remains the independent white-glyph alpha pass. An optional `colorData` supplies a separate, equally sized RGBA8 glyph-color image with top-down rows and straight RGB; its alpha bytes exactly match R8. The color pass uses the same font, layout, wrapping, tabs, scroll, and clipping. Only Life uploads the extra color texture. A color-only change updates future input without re-emitting or repainting existing cells. Mask-only callers use the standard neutral text color as a fallback.

The renderer always uploads the newest raw R8 image. When `emissionRevision` changes, it creates an additive image from positive alpha changes against the previous uploaded image and submits that image once to Life. Every raw update refreshes the comparison baseline, including whitespace-only and scroll captures. An unchanged emission revision suppresses the Life pulse even if whitespace moved the visible letters. Empty or removed pixels have no destructive effect. Callers omitting `emissionRevision` fall back to image-difference detection.

`reseedLife()` remains an explicit operation for a manual restart. A future compile-triggered restart can call it separately; compiling currently preserves the world.

Snapshots also carry `scrollRevision`, initially zero and advanced only after a successful capture at different scroll offsets. A changed uploaded scroll revision takes precedence over an additive emission: the renderer seeds once from the complete newest mask. Repeated captures at the same position, or the first snapshot establishing the baseline, do not refresh an existing world. Failed captures do not consume a scroll change.

## Custom GLSL

Use `mainImage(out vec4 fragColor, in vec2 fragCoord)` in the `shader.glsl` tab and run with Ctrl/Command + Enter. A compilation failure preserves the last working program.

| Uniform | Contents |
| --- | --- |
| `iChannel0` | Raw glyph coverage; sample `texture2D(iChannel0, uv).r`. |
| `iChannel1` | Persistent field: narrow energy in red, broad diffusion in green, recent input pulse in blue. |
| `iChannel2` | Discrete Life state: red is binary occupancy; other channels carry decorative history. |
| `iChannel3` | Persistent Life colors in RGB, separate from occupancy and trails. |
| `iChannelResolution[0]` | Mask dimensions. |
| `iChannelResolution[1]` | Feedback-buffer dimensions. |
| `iChannelResolution[2]` | Life-grid dimensions. |
| `iChannelResolution[3]` | Life-color-grid dimensions, matching the Life state grid. |
| `u_lifeGeneration` | Completed Conway generations since the last scroll refresh, manual restart, or initial allocation. |
| `u_maskRevision` | Uploaded snapshot revision. |
| `u_emissionRevision` | Uploaded non-whitespace/typography input revision; unchanged for whitespace-only edits and identical captures. |
| `u_maskAge` | Simulation time since the latest uploaded mask; freezes when paused. |
| `u_textInfluence` | Text influence control, 0–1. |
| `iTime`, `iResolution`, `iMouse`, `iFrame` | Animation clock, drawing-buffer size, pointer, and submitted-frame index. |
| `u_palette` | Violet=0, mint=1, amber=2, monochrome=3. |

The WebGL 2 wrapper adapts the `texture2D` spelling to `texture`; presets also compile through the WebGL 1 wrapper. UVs have the usual GLSL bottom-left origin. The renderer explicitly flips the top-down R8 rows during upload.

Referencing `iChannel2` or `iChannel3` in a custom shader activates the Life simulation. Its programs and state/color textures are allocated only when needed. Switching away holds the world without stepping its generations; new text can still add cells to that retained world. Returning to Life resumes it.

## Separation and limits

The canvas belongs only to `.editor-stage`. Tabs, menus, status, playback, and inspector remain separate opaque surfaces. A fixed neutral scrim blends the shader at 50% by default; text stays fully opaque.

`commitInput()` does not wait for rendering or read back shader pixels. `TextMaskCapture` owns snapshot timing, and `ShaderEngine` owns animation, texture upload, and feedback. A single pending-image slot lets newer snapshots supersede older ones without queuing an unbounded backlog.

The browser implementation still shares the main thread for text work, the Canvas2D capture, and WebGL submission. Its CPU readback is of the dedicated text pass only. It is not a native asynchronous GPU queue or worker-isolated renderer. A native implementation can produce the same single-channel glyph surface directly on the GPU and avoid the browser raster-copy step.

The mask represents the visible editor viewport, including its current scroll and wrapping, at CSS-pixel scale, capped to a maximum dimension of 1536 pixels. Offscreen document text is not included. Complex bidirectional script shaping can differ from native textarea layout; the monospace code-text path preserves graphemes, tabs, and antialiased coverage.

Game of Life receives one-time additions from captured glyph pixels and keeps its own persistent state. Dynamically varying composition alpha remains a future experiment.

## Run and files

Serve `dist/` with any static HTTP server. There is no build step, external CDN, API key, or runtime package dependency.

- `dist/index.html`: editor shell, mask inspector, and controls.
- `dist/style.css`: responsive layout and composition.
- `dist/app.js`: text editing, menus, capture/renderer integration, and setup import/export.
- `dist/text-mask.js`: transparent glyph capture, alpha extraction, layout, and cadence.
- `dist/text-colors.js`: syntax-color runs from the rendered editor, with exact source offsets.
- `dist/shader-engine.js`: R8 input, WebGL lifecycle, feedback surfaces, Conway state, and eight shaders.
- `dist/matrix-shader.js`: Matrix rain fragment source.
- `dist/fonts/`: local interface and monospace fonts.

Document text is not sent to a server. Browser persistence and setup export include the document, shader source, capture cadence, and visual controls. The v1 browser session is migrated without changing its document text; Aurora becomes the initial v2 effect. The renderer recreates the mask from the document after a reload.

## Validation

`npm test` runs **57 regression checks** covering the alpha-channel contract, deferred/coalesced input capture, strict three-second cadence, manual hold, hidden-page scheduling, emission identity, whitespace and typography changes, one-time additive uploads, preserved clocks/generations, explicit restart behavior, and captured-scroll refreshes without external dependencies. Color checks also cover rendered DOM source offsets, straight RGBA upload, independent R8 coverage, matching state/color buffer updates, lazy allocation, scroll/manual reseeding, resize, context restoration, and disposal.

The text pass was also exercised through a real Canvas2D implementation with the bundled font: empty text produced all-zero bytes, same-length different letters produced different masks, partial-alpha edges were preserved, and one-line scroll translated the mask exactly.

The companion color pass passes eight real Canvas2D scenarios with byte-identical R8 results compared with the previous version. Every covered pixel across all eleven editor colors in the antialiased default font keeps its source RGB: 3,476 pixels, zero color mismatches. Token ownership prevents low-alpha premultiplication rounding from confusing similar syntax colors. Block-font sizes 16 and 32, wrapping, tabs, CRLF, two-axis scroll, and empty input are covered in `docs/glyph-color-validation.json`.

The original six effects and the feedback pass compiled and linked through GLSL ES 1.00 and 3.00 wrappers in a real EGL/OpenGL ES context. Rendering with the actual captured R8 image, a translated version containing identical glyph coverage, and a blank mask confirmed image dependence for every effect. Empty input plus cleared history exactly matched the blank baseline. Measurements are in `docs/shader-validation.json`.

The expanded eight-effect engine, feedback, Life seed, and Life step also compiled and linked through both wrappers (22 program combinations). Actual GPU Life steps passed block, blinker, glider, finite-boundary, empty-world, and decorative-channel-isolation checks, plus forty generations against an independent CPU reference. The production glyph fixture seeded 2,699 live cells and evolved to 1,643 at generation 30. See `docs/life-validation.json` and `docs/life-generation-30.png`.

The persistent-emission update adds an independently validated Life addition pass. Its 24 program/wrapper combinations compiled and linked. GPU checks confirm an evolved glider survives new input, blank/zero-delta emissions leave every state byte unchanged, the combined world follows the independent Conway reference, and resizing copies cells and trails exactly. The real glyph fixture retained all 1,643 existing live cells while adding another 228. Explicit seeding still replaces the world when manually requested. See `docs/life-emission-gpu-validation.json`, `docs/life-emission-runtime-validation.json`, and `docs/compile-validation.json`.

The syntax-color update compiles and links all **30 current program/wrapper combinations**: eight displays, feedback, three occupancy passes, and three color passes through GLSL ES 1.00 and 3.00. Eleven additional GPU checks cover exact glyph RGB, antialiased inputs, black live cells, byte wraparound including `255 + 1 = 0`, direct collisions, same-color inheritance, mixed births, trails, paired resizing, and reseeding. Thirty random generations match an independent CPU reference byte-for-byte for both occupancy and RGB. The actual syntax-colored glyph fixture seeds 2,281 cells in six source colors. See `docs/life-color-gpu-validation.json`, `docs/life-color-runtime-validation.json`, and `docs/life-syntax-colors.png`. The seven other display sources, generic feedback, and all three occupancy shaders remain byte-for-byte unchanged.

Matrix renders with the same glyph fixture confirmed that moving identical letters changes the rain distribution and advancing time moves the rain. An empty mask with cleared feedback produced a spatially uniform dark result. See `docs/matrix-validation.json` and `docs/matrix-preview.png`. The original six preset sources and feedback source remain byte-for-byte unchanged.

These are real raster/GPU and code checks, not browser UI tests. Browser visual alignment and interaction QA were unavailable because the managed preview service was not running; the previous private-site browser visit required a fresh sign-in.

The block WOFF2 was decoded and rasterized with a real Canvas2D engine. All printable ASCII characters have equal advances, and its production capture masks change with the selected font and size. Font metadata checks are in `docs/font-validation.json`; typography capture results are in `docs/typography-validation.json`.

## References

- https://github.com/microsoft/edit
- https://github.com/t4ce/TRUEOS
- https://github.com/t4ce/TRUEOS-Buildins/tree/master/edit
- https://github.com/t4ce/TRUEOS-Blueprints/tree/main/apps/shadertoy
