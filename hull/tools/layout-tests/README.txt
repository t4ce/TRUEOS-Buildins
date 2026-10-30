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
