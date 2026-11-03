Hull host sandbox — capture once, replay offline

This builds one ordinary Linux executable from Hull's production rp5 parser,
QuickJS engine/actor, lazy DOM/window bridge, CSS collection, selectors, Taffy
layout and diagnostic SVG export. It does not build the kernel or built-in
package, boot TRUEOS, start a VM, open Winit, or submit GPU work.

From TRUEOS-Buildins:

  python3 hull/tools/sandbox/sandbox.py capture 'https://example.com/' /tmp/hull-page
  python3 hull/tools/sandbox/sandbox.py replay /tmp/hull-page

Capture is the only command which fetches page resources. It requests HTML for
page navigation (some sites return 404 without that Accept header); resource
fetches retain their general Accept header. It saves page.html, capture.json,
and bounded CSS resources, using the production parser/CSS collector
to discover references and imports. It never evaluates JavaScript. Images,
media, fonts, iframes and arbitrary page navigation are not fetched. --scripts
also saves external JavaScript but does not enable it in replay. HTTP errors
are recorded; there is no login, cookie handling or access-control bypass.

To capture an ESM entry and its static graph without executing it:

  python3 hull/tools/sandbox/sandbox.py capture 'https://example.com/' /tmp/hull-page \
    --scripts --module is-number@7.0.0
  python3 hull/tools/sandbox/sandbox.py replay /tmp/hull-page --modules --turn-ms 0

--module accepts a bare npm specifier or HTTPS URL and can be repeated for dynamic
entry points. QuickJS's parser discovers static dependencies; arbitrary computed
imports cannot be enumerated at capture time. Replay diagnostics identify missing
URLs, which can be added as seeds. esm.sh is primary, jsDelivr +esm is fallback
for npm entries. Site-relative /_app/... requests remain on the site's origin.
See hull/MODULES.md for source preparation, provider rules and API limits.

For a browser-saved HTML file (preserves the exact bytes being investigated):

  python3 hull/tools/sandbox/sandbox.py capture 'https://original/page/' /tmp/hull-page \
    --html /path/to/saved-page.html --no-css

Resources can be imported manually in capture.json:
  {"version":1,"url":"https://original/page/","resources":{
    "https://original/page/style.css":{"status":"ok","kind":"css","path":"style.css"}
  }}
Resource paths must resolve inside the capture directory. Use --refresh to
replace an existing capture; ordinary replay never refreshes it.

The replay output defaults to CAPTURE/run:

  layout.svg       Production diagnostic text/box SVG, not a browser screenshot.
  layout.json      Production layout export.
  scripts.json     All sources with node IDs, type, origin and per-source outcome.
  scripts/*.js     Exact originally extracted inline source bytes.
  admitted/        Actual selected/replaced source bytes and their outcomes.
  trace.jsonl      Actor events, errors, console messages and host-turn timings.
  console.log      Console output and full exception messages/stacks.
  report.json      Configuration, cutoff, resource requests and layout summary.

Look at the exact source around a failing stack line:

  python3 hull/tools/sandbox/sandbox.py show /tmp/hull-page --node 1626 --line 5

Copy its scripts/NNN-node-1626.js file to a local working file, edit it, then:

  python3 hull/tools/sandbox/sandbox.py replay /tmp/hull-page \
    --only-node 1626 --replace 1626=/tmp/working.js --turn-ms 0
  python3 hull/tools/sandbox/sandbox.py watch /tmp/hull-page \
    --replace 1626=/tmp/working.js --turn-ms 0

Watch reruns when captured files, replacements or Hull Rust/JS sources change.
The host executable is rebuilt incrementally only through its own host Cargo
workspace/cache. Cargo runs outside Blueprint configuration, with an explicit
Linux target, pinned toolchain and --offline. The first build needs the existing
Rust dependency cache; subsequent runs require no downloads.

Useful comparisons:

  --no-js          Static HTML/CSS baseline, including SVG.
  --no-svg         JavaScript diagnostics only; skip layout/SVG.
  --turn-ms 20     Default: Hull's current idle delay between actor turns.
  --turn-ms 0      Fast headless scheduling; exposes elapsed-wait effects.
  --no-window      Reproduce an environment without window globals.
  --external       Legacy experiment: admit captured external bodies without graph loading.
  --modules        Enable production source/module loading from captured tickets only.
  --prelude FILE   Experiment: execute local JS before page scripts (node 0).
  --only-node N    Experiment: execute one collected source independently.
  --replace N=FILE Experiment: substitute a source without editing the capture.
  --out DIR        Separate artifacts for comparisons.
  --width/--height Content viewport, initially 1280x720, not whole native frame.

Production retains the active 250 ms allowance and per-call 25 ms interrupts.
Resource-enabled initial pages exclude bounded resource waiting from the 250 ms
elapsed window (downloads/preparation up to 5 s, 5.25 s overall initial cap). The child
process additionally has a 30 s host
timeout, 15 s CPU limit, 1 GiB address-space limit and 64 MiB per-file limit on POSIX.
These are practical exploration limits, not a general OS security boundary.
There is no general-purpose guest fetch/XHR, filesystem, shell, timer or process
API. Imports request host-managed HTTPS JavaScript sources; replay fulfills them
only from captured files and never contacts the live CDN.

DOM-backed replay also installs Hull's native rjqry admission as $/jQuery. For
example $('#message').text('ready').addClass('ready') mutates the same rp5 tree
before its frozen first-render snapshot, and $.param/$.escapeSelector execute in
Rust. This is a DOM-focused subset, not full jQuery: no ready scheduler, HTML
construction, seeded filtering, effects, plugin helpers or Ajax is admitted.
See hull/rjqry/README.md for its exact API/work limits. JS callbacks still run
in QuickJS; the JSON host bridge remains. No speedup is claimed without a benchmark.

Replay never makes live web requests. CSS tickets read captured files only;
missing resources fail explicitly. Only inspect/capture/replay artifact code in
the host accesses files. No downloaded JavaScript runs during capture.

Host fixtures (no live network):

  python3 hull/tools/sandbox/test_sandbox.py
