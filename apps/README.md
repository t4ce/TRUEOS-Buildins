# Apps start scripts

Apps uses the kernel's native HTTP/TLS fetch service. Each request is bounded
to 90 seconds, with a 1 MiB catalog limit and a 128 MiB archive limit before
body accumulation. Apps retains catalog parsing and its SHA-256 verification
policy; verified archives are durably published through typed filesystem writes
before AppDB import or launch. Dropping a download cancels its native operation.
Diagnostics report endpoint categories, elapsed time, native status codes and
completed byte counts; transport details belong to the kernel service.

Host parser, mouse, script and native transport ownership tests can be run with:

```sh
CARGO_TARGET_DIR=../../TRUEOS/tgt/apps-boundary-tests cargo +nightly-2026-07-10 test --manifest-path tests/Cargo.toml --target x86_64-unknown-linux-gnu
```

Apps consumes `vFile:launch` before opening its interactive UI. Each line
is `pull NAME_OR_ID` or `launch NAME_OR_ID`; blank lines and comments are ignored.
It resolves the catalog entry, downloads and verifies the blueprint, and launches
it through `trueos::vshell::launch_with_destination`.

To supply the downloaded app with its own one-shot start script, add ` -- ` and
one command. For example, OSM submits:

```text
launch --sh3 Frog -- weather 13.8278 51.4713
```

Apps forwards `weather 13.8278 51.4713\n` to Frog's `vFile:launch`.
`--sh3` creates one kernel-owned Shell3 window and enters the downloaded app's
Matrix slot through the normal `§slotid` path. The flag belongs before the app
selector; it is not part of the child's script. Without it the launch inherits
the caller's Shell3 frontend, including SSH. UI4 callers can start the pullbot
with `launch_with_destination("apps", script, LaunchDestination::Headless)`.
Existing requests without a forwarded command still launch with an empty script.
Multiple request lines retain their order and duplicates; a bad request does not
prevent later requests from being processed.

Cold-boot autostart always invokes the pullbot with `pull --headless NAME_OR_ID`.
`--headless` submits the downloaded app without a Shell3 frontend, using the
shared host launch queue. It cannot be combined with `--sh3`. Autostart uses
`online_selector` when set, otherwise the archive name without `.bp`; it never
launches a cached copy of the requested app. A single-line `launch_script` is
forwarded after ` -- `. Guest arguments and named instances are unsupported by
this pullbot protocol and autostart reports an error if either is configured.

## Fetch without launching

The one-shot startup command `install NAME_OR_ID` uses the normal
`https://trueos.eu/apps` catalog, download verification and AppDB import, then
exits without starting the downloaded app. IDE compiler jobs use this when
their compiler is missing from AppDB and then retry the original build.

## App bundles

`TRUEOS-Blueprints/appbundle.json` declares one-way runtime requirements:

```json
{
  "version": 1,
  "bundles": {
    "codex.bp": { "requires": ["sqlite.bp"] }
  }
}
```

Keys and requirements are Blueprint archive names. The key is the app to start;
requirements are installed, and the app starts its services when needed.
Requesting SQLite alone does not request Codex. Requirements may have their own
requirements; shared members are fetched once, and cycles are rejected.

The host AppDB seed builder validates and stores this JSON in the database's
`metadata` table under `appbundle.json`. It is separate from the built-in app
selection. Reprepare the kernel and builtins after editing the bundle file.
Apps reads the embedded definition through `vFile:appbundle.json`; there is no
metadata file to fetch from the app server.

Interactive launch, Space-to-install, and pullbot `pull`, `launch` and `install`
all resolve every member before downloading anything. Every member must be
available, downloaded, verified and saved before the bundle is imported in one
AppDB transaction. A failed request preserves previous AppDB entries and never
starts the requested app. Staged files may remain on disk for a failed request.
The bottom TUI status line names the unavailable member and reason, for example:
`codex: required sqlite.bp unavailable in app catalog`.
Pullbot reports the same message through its normal error output.

`vFile:appdb-install` accepts one pathname or a newline-separated batch of up to
64 paths. The kernel resolves and authorizes each path, checks every Blueprint,
and commits the apps, hashes and update dates together. Missing bundle
requirements reject the transaction. Importing a bundle does not start helpers.
