# TRUEOS Share

A read/write WebDAV buildin using `dav-server` 0.11.0 and
Hyper HTTP/1. It serves its own persistent `TRUEOS_APP_FS_ROOT` (normally
`apps/share`) on TCP port **4918**. It does not require `websys` or the kernel's
port-80 filesystem service.

This first version deliberately has **no authentication and no TLS**. Every
client that can reach the port can read and modify the exported directory.

## Build for TRUEOS

From the TRUEOS repository, the normal native build includes Share:

```sh
cargo rigreal --native-only
```

The app is registered as a buildin in `buildins.json`, with its sources in
the adjacent TRUEOS-Buildins repository's `share` directory.
The default TRUEOS startup manifest launches it headlessly from the embedded
AppDB on cold boot, without fetching it from the online app catalog.
It uses the normal VMX Blueprint execution path, without a guest Linux system.
The root is created when the server starts. Files live in TRUEOSFS independently
of the server process.

## Connect from Linux

After launching the Blueprint on TRUEOS, enter this location in GNOME Files:

```text
dav://TRUEOS-IP:4918/
```

Or mount and list it using GIO (requires the GVfs WebDAV backend):

```sh
gio mount dav://TRUEOS-IP:4918/
gio list dav://TRUEOS-IP:4918/
```

For an ordinary filesystem mount, use the existing `davfs2` client with
`http://TRUEOS-IP:4918/` as its server URL. A browser can also browse and download
files from that HTTP URL.

## Local development and tests

The host manifest builds the same HTTP driver and desktop filesystem backend
against upstream Tokio/Hyper and the vendored DAV protocol implementation.
The native storage fixtures compile the actual native backend against a
deterministic in-memory native service, including publication failures and races.

```sh
cargo test --manifest-path share/host/Cargo.toml
cargo test --manifest-path share/native-tests/Cargo.toml
TRUEOS_SHARE_BIND=127.0.0.1:4918 cargo run \
  --manifest-path share/host/Cargo.toml -- /tmp/share-test
```

`[ROOT]` selects a different serve directory. Without an argument the server uses
`TRUEOS_APP_FS_ROOT`, or `share-data` on a development host. `TRUEOS_SHARE_BIND`
overrides the default `0.0.0.0:4918` listener.

## Supported operations and current limits

- OPTIONS, PROPFIND, GET/HEAD with byte ranges, PUT, MKCOL, COPY, MOVE, DELETE.
- LOCK/UNLOCK use real in-memory WebDAV locks, lost when the server restarts.
- HTTP directory listings are enabled. Symlinks are hidden and rejected by the
  backend; the share root itself cannot be deleted or moved through WebDAV.
- Native PUT stages the whole body without changing storage, validates its
  length, and waits for explicit checked publication. Dropping a handle never
  writes. Failed or interrupted bodies leave the previous record intact.
- Replacing a native file retains its stored type, key and record identity.
  New foreign uploads are BLOB records, regardless of filename or supplied MIME.
  GET, HEAD and PROPFIND expose the stored MIME. COPY uses a coherent source
  snapshot and its type; MOVE retains the original record's identity and key.
- Conditional creates and partial/append updates are checked again at commit.
  An intervening change fails the conditional write instead of publishing a
  stale buffer. Ordinary full PUT retains last-writer behavior.
- Files are limited to **4 GiB** (4,294,967,296 bytes). The native backend buffers
  whole files and uses fallible growth; four HTTP connections can run at once.
  The kernel must include the matching async-filesystem read/write caps.
- Native metadata currently exposes size and type, without timestamps or ETags.
- MKCOL reserves only the requested absent directory under an existing parent.
  File and empty-directory deletion check kind and emptiness under the native
  namespace lease. COPY and MOVE refuse a raced destination. DAV overwrite
  handling may delete a destination before COPY/MOVE; those complete operations
  do not promise transactional replacement.

Host tests cover file operations, ranges, overwrite, locks, confinement, size
limits and the HTTP socket driver. Native fixtures cover coherent reads, typed
replacement, commit failure and retry, cancellation, conditional races, COPY/MOVE
identity and malformed upload bodies. Native target and hardware verification
are recorded separately by the TRUEOS build audit.
