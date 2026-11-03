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
  storage.json      Navigation-local document.cookie pairs at SVG capture time.
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
  --turn-ms 20     Default: delay between UI-side report/snapshot polls.
  --turn-ms 0      Poll promptly; native JavaScript runs independently either way.
  --no-window      Reproduce an environment without window globals.
  --external       Legacy experiment: admit captured external bodies without graph loading.
  --modules        Enable production source/module loading from captured tickets only.
  --prelude FILE   Experiment: execute local JS before page scripts (node 0).
  --only-node N    Experiment: execute one collected source independently.
  --replace N=FILE Experiment: substitute a source without editing the capture.
  --out DIR        Separate artifacts for comparisons.
  --width/--height Content viewport, initially 1280x720, not whole native frame.

Production retains the active 250 ms allowance and initial render cutoff, with
per-call 125 ms interrupts. The per-call allowance does not extend the initial
render window or authorize later DOM changes to publish a new frame.
JavaScript sources and CSS sheets are capped at 4 MiB each; their aggregate
production budgets are 8 MiB each. Capture and replay use the same per-file cap.
DOM wrappers support EventTarget and document DOMContentLoaded runs, with its
Promise jobs, before the first snapshot. There is no automatic window load event
or HTML on* handler binding. Native keyboard/pointer records remain on hullInput;
Winit window resize/focus/blur records additionally reach window DOM listeners.
Resource-enabled initial pages exclude bounded resource waiting from the 250 ms
elapsed window (downloads/preparation up to 5 s, 5.25 s overall initial cap). The child
process additionally has a 30 s host
timeout, 15 s CPU limit, 1 GiB address-space limit and 64 MiB per-file limit on POSIX.
These are practical exploration limits, not a general OS security boundary.
There is no general-purpose guest fetch/XHR, filesystem, shell or process
API. Imports request host-managed HTTPS JavaScript sources; replay fulfills them
only from captured files and never contacts the live CDN.

URL and URLSearchParams are available in inline scripts and loaded modules,
including new URL(relative, import.meta.url). They use Hull's Rust parser and live
query binding with finite string/pair limits. URL construction performs no fetch
or navigation. JAVASCRIPT.md describes the admitted API.

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

Cookies are a fresh empty in-memory jar for every navigation. Replay never reads
storage.json; it writes this diagnostic beside layout.svg only when saving SVG.
Production similarly captures cookies when preparing its one diagnostic SVG,
then persists both after publishing the frame. Later JavaScript may continue and
change the live jar; those changes do not rewrite the saved initial snapshot.

Cookies and localStorage use one bounded navigation-local box, with internal
cookie: and localStorage: key prefixes and one cumulative 1 MiB write allowance.
The storage.json artifact contains both namespaces and is never restored.

setTimeout/clearTimeout use production actor deadlines and the page's existing
execution budget (256 pending timers, function callbacks only). Future timers
do not delay the frozen initial snapshot. Replay stops at initial completion;
it does not keep running merely to wait for future timers.

Page video.play() requests are recorded as video_requests in report.json.
The sandbox rejects the separate native-player handoff explicitly; it never
fetches media or opens a player. Native playback lifecycle is host-mocked in
js-tests and TRUEOS/tools/testpy/test_browser_video_handoff.py.
