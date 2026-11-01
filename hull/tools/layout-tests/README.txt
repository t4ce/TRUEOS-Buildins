Host regressions run production CSS collection, Taffy layout, image admission and
sprite projection, viewport clipping, script inspection, and snapshot scheduling.
Kernel network/image/UI4/filesystem interfaces are mocked; no external network or
disk writes occur in the tests.

Run outside the Blueprint Cargo configuration:
  cd /tmp
  CARGO_TARGET_DIR=/home/t4ce/Repositories/TRUEOS/tgt/hull-layout-tests cargo test --manifest-path /home/t4ce/Repositories/TRUEOS-Buildins/hull/tools/layout-tests/Cargo.toml

Image policy:
- The first base[href] resolves img src. HTTPS URLs retain query strings and
  discard fragments; repeated canonical URLs share one of eight asset tickets.
- Every paintable img retains a frozen box, including invalid/overbudget sources.
  CSS dimensions precede HTML hints; a missing dimension uses aspect ratio and
  an unspecified image starts at 300x150. Dimensions fit within 1024x1024.
  Percentage width/height stays container-relative; percentage min/max image
  constraints are projected against the viewport under the same safety cap.
- The initial layout/text frame publishes before image admission. Each event-loop
  turn admits at most one asset and polls at most two statuses. Pixels stay in the
  kernel; ready handles only trigger repaint. Navigation releases/reset generations.
- One ordered sprite batch crops both rectangle and UVs to the viewport. Pending,
  failed, unsupported and overbudget sources remain neutral placeholders.
- CSS collection permits 16 external sources, 512 KiB per sheet and 2 MiB total,
  freezes collected styles after two seconds, and never relayouts on completion.
  External scripts are recorded without fetching or execution.
- The SVG snapshot is prepared without filesystem activity, then polled once per
  event-loop turn after publication. Failure or the five-second timeout affects
  neither live layout nor navigation. Snapshots larger than 8 MiB are omitted.

Quad painting:
- actualBgQuad is the default: solid CSS background/background-color fills,
  including currentColor and color alpha, in DOM order. Backgrounds are not
  inherited. Hidden, zero-opacity and display:contents boxes do not paint.
- F2 switches to pinkQuadDbg and back without downloading or relayout. The
  diagnostic pass keeps every element bounding box and the original magenta
  [255, 0, 255, 33] source-over stacking opacity, regardless of CSS backgrounds.
- Both passes share viewport scrolling/clipping and draw before images/text.
  CSS background images, gradients, rounded clipping, z-index stacking contexts
  and partial-opacity groups are not implemented by this solid background pass.
- The SVG snapshot remains a diagnostic layout export saved after publication.

Native border pass:
- Runs over either fill pass, before images/text, using PotatoStamps' V2
  LINE_LOOP geometry type. Borders default to zero: only boxes with a positive
  used CSS border width contribute a loop. Each such diagnostic box contributes one
  loop: four unrepeated vertices for square corners, plus one vertex/segment
  per nonzero corner radius (eight segments for four rounded corners).
- Rounded corners are straight chords between shortened sides. Pixel and
  percentage radii are resolved per box and reduced together if they overlap.
- Visible borders keep the native 3px width. The uniform loop color defaults to
  black; authored border/border-color/border-top-color uses the cascaded top
  border color, including currentColor and alpha. Per-side colors/styles and
  authored widths are intentionally not reproduced by this baseline pass.
- Original offscreen boundaries are retained for GPU clipping, so scrolling
  does not invent border edges at the viewport. Large loop worklists submit
  in batches of at most 340 native draws with LOAD_COLOR, preserving the
  existing fill and order. The 600-entry ABI array is only wire capacity.
- The kernel and built-in vGPU facade must both include the V2 LOAD_COLOR flag.

Hyperlinks with href default to blue; authored CSS color overrides that default.

Custom properties and canvas:
- Custom --properties participate in the existing cascade and inherit by scope.
  Lightning CSS substitutes var() tokens and reparses supported typed values;
  nested fallbacks are supported, and cyclic definitions become invalid.
  Resolution is bounded to 32 dependency/nesting levels and 8192 tokens per value.
- The html background paints the page canvas. A transparent html background
  propagates the body background instead. Its alpha composites over white once;
  the propagated element does not paint a duplicate background over itself.
- Live actualBgQuad painting and the SVG canvas use the same resolved color.

Default link navigation:
- A normal Hull launch is idle at its URL prompt. There is no initial fetch.
- vFile:launch may contain one `open <HTTPS URL>` directive. The existing URL
  queue starts it after window creation; manual URL entry still navigates here.
- Left-button press/release on the same painted anchor opens a fresh Hull using
  launch_with_destination("hull", script, NewShell3), including same-domain links.
  Relative hrefs use the first base[href] and document URL. Image and nested-text
  anchors have document-space hit boxes, clipped and scrolled like their paint.
- Chrome, hidden content, unsupported schemes, drags and canceled pointers do
  not launch. Fragments retain the current URL input policy (no anchor scrolling).
