# Hull JavaScript: first step

Hull starts one idle Tokio local task with its event loop. It uses the same
thread as Hull, with no additional OS worker or browser environment. The
QuickJS engine is a separate small crate with an opaque native boundary;
no qjs workbench or web-experiment code is imported.

Page extraction only stores script bytes and references. It neither inspects
JavaScript source nor runs it. `javascript::Runtime::run()` is the explicit
execution capability; Hull does not currently call it from navigation, page
loading, UI events or a shell command. There is no automatic script execution.

When explicitly requested, one available source is evaluated per event-loop
turn. Classic JavaScript and native modules use their metadata type; data
script types are skipped. External references remain unfetched. Modules have
no import loader. Promise jobs run before the following source, at most sixteen
per turn. There are no DOM, console, filesystem, network or timer bindings.
Native exceptions are reported; unhandled Promise rejection tracking is not
installed yet.

Navigation replaces the dormant batch and drops the old context, globals and
pending jobs before any further evaluation. The new page requires a new explicit
run request. Closing Hull drops the task and its engine as well.

The engine defaults to a 32 MiB heap, 1 MiB native stack and a 25 ms cooperative
execution interrupt, with at most 1 ms additional cooperative diagnostic time.
This is not a hard UI-latency guarantee: parsing, GC and native builtins can
overrun the interrupt slice. Interrupted evaluation aborts; it does not resume
on the next turn. These controls do not make an in-process VM an OS sandbox.

Host checks: `tools/js-tests/README.txt` exercises the production task and script
extractor. The engine crate has separate native tests. `tools/layout-tests`
continues to check Hull's existing rendering path. Built-in packaging uses
`../tools/build.py --app hull`; it does not launch Hull.
