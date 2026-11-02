# Trauma

[![Crates.io](https://img.shields.io/crates/v/trauma.svg)](https://crates.io/crates/trauma)
[![Documentation](https://docs.rs/trauma/badge.svg)](https://docs.rs/trauma/)
[![ci](https://github.com/rgreinho/trauma/actions/workflows/ci-rust.yml/badge.svg)](https://github.com/rgreinho/trauma/actions/workflows/ci-rust.yml)

Tokio Rust Asynchronous Universal download MAnager

## Description

This TRUEOS fork provides asynchronous HTTP(S) downloads as a library and a
terminal application. The default build keeps download, retry, and execution
logic without terminal progress rendering or HTTP telemetry.

![trauma v3 default UI](assets/trauma-v3-default.png)

### Features

- Library and TRUEOS terminal application
- HTTP(S) downloads with [rustls]
- Support download via proxies
- Download files via providing a list of URLs
  - Automatically use the remote file names
  - Ability to rename downloaded files
- Ability to configure the download manager
  - Download directory
  - Maximum simultaneous requests
  - Number of retries
  - Resume downloads (if supported by the remote server)
  - Custom HTTP Headers
- Asynchronous w/ [Tokio]
- Optional progress bar w/ [indicatif] (`terminal-progress`)
  - Display the individual progress
  - Display the total progress
- Ability to customize the optional progress bars
  - Customize the format
  - Customize the progression style
  - Leave them on the screen or clear them upon completion
  - Hide any or both of them
  - Use pre-configured styles

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
trauma = "3"
```

### Features

By default, `trauma` enables [rustls] for TLS support. Streaming and HTTP/2
remain enabled. Optional capabilities are explicit:

| Feature | Capability |
| --- | --- |
| `terminal-progress` | Indicatif bars and the `StyleOptions` / `ProgressBarOpts` APIs |
| `retry-tracing` | Retry middleware tracing diagnostics |
| `socks` | SOCKS proxies |
| `system-proxy` | Automatic platform proxy discovery |

Explicit HTTP proxy configuration through `download_with_proxy` remains
available without the SOCKS or system-proxy features. The lean build returns
normal download summaries without drawing progress bars. Applications can
choose their presentation; this fork does not yet expose a progress-event API.

For the existing progress styles, enable `terminal-progress`:

```sh
cargo run --features terminal-progress --example with-style
```

The examples use a single-thread Tokio runtime. Their error reporting, tables,
and partial-download fixture use standard Rust facilities without extra dev
libraries. See [PORT_PROBE.md](PORT_PROBE.md) for platform build evidence and
remaining transfer-correctness limitations.


The vendored Reqwest fork maps the `default-tls` compatibility alias to rustls:

```toml
[dependencies]
trauma = { version = "3", default-features = false, features = ["default-tls"] }
```

### Platform types and verification

Portable traits use `core`; owned strings, collections, and shared pointers use
`alloc`. `trauma::path::{Path, PathBuf}` selects TRUEOS's
`trueos::platform::path` facade on TRUEOS and a host fallback elsewhere. The
current platform facade reexports std-compatible types; this is not a full
`no_std` port. OS I/O and current-directory access still use `std`.

Build the actual platform application from the sibling Blueprint checkout:

```sh
TRUEOS_BLUEPRINT_SKIP_APPS_PUBLISH=1 cargo bp trauma
```

From this checkout, run the supplemental host checks:

```sh
python3 tools/check_host.py
```

The script checks the real library, examples, and loopback tests using vendored
Tokio/Mio and an upstream Reqwest HTTP backend without TLS. It tests both lean
and optional terminal/proxy feature combinations, plus documentation examples.
It does not execute the TRUEOS runtime or validate platform TLS. The actual
TRUEOS all-features compile check and pack results are recorded in
[PORT_PROBE.md](PORT_PROBE.md).

## Quick start

```rust
use trauma::{download::Download, downloader::DownloaderBuilder, Error};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    let reqwest_rs = "https://github.com/seanmonstar/reqwest/archive/refs/tags/v0.11.9.zip";
    let downloads = vec![Download::builder().url(reqwest_rs)?.build()];
    let downloader = Downloader::builder().directory("output").build();
    downloader.download(&downloads).await;
    Ok(())
}
```

More examples can be found in the [examples](examples) folder. They are well
commented and will guide you through the different features of this library.

## Why another download manager

Before starting this project, I spent some time searching the internet, trying
not to reinvent the wheel. And I did find a bunch of interesting existing
projects!

However they are almost all abandoned:

- DLM: <https://github.com/agourlay/dlm>
  - Active, but is just a binary/CLI tool
- Snatch: <https://github.com/derniercri/snatch>
  - Inactive since Sept '17
  - Recommend switching to [Zou]
- Zou: <https://github.com/k0pernicus/zou>
  - Inactive since Oct '17
- Duma: <https://github.com/mattgathu/duma>
  - Inactive since Nov '20
- Siwi: <https://github.com/rs-videos/siwi-download>
  - Inactive since Mar '21
- Downloader: <https://github.com/hunger/downloader>
  - Dying project
  - No answers to issues/PRs
  - Only automated updates are being merged
  - No release since Feb '21

As a result, I decided to write `trauma`.

[indicatif]: https://github.com/console-rs/indicatif
[rustls]: https://rustls.dev/
[tokio]: https://tokio.rs/
[zou]: https://github.com/k0pernicus/zou
