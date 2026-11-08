//! Execute an ordered one-shot install/launch batch through native services.
use super::script::{Request, requests};
use alloc::{format, string::String};

pub fn report(message: &str) {
    let _ = trueos::logl::log_record(
        trueos::logl::level::IMPORTANT,
        "apps",
        format_args!("apps pullbot: {message}"),
    );
}
pub async fn drain(bytes: &[u8]) {
    let script = match core::str::from_utf8(bytes) {
        Ok(script) => script,
        Err(_) => {
            report("startscript is not UTF-8");
            return;
        }
    };
    let requests = requests(script);
    if requests.is_empty() {
        report("drained: 0 requests");
        return;
    }
    let catalog = match super::fetch::bytes(super::CATALOG, super::fetch::CATALOG_LIMIT).await {
        Ok(bytes) => match core::str::from_utf8(&bytes) {
            Ok(html) => super::catalog::parse(html),
            Err(_) => {
                super::error_output("catalog is not UTF-8");
                report("catalog is not UTF-8");
                return;
            }
        },
        Err(error) => {
            super::error_output(&format!("catalog: {error}"));
            report(&format!("catalog: {error}"));
            return;
        }
    };
    let mut queued = 0;
    let mut failed = 0;
    for request in requests {
        let result = async {
            let Request {
                selector,
                install_only,
                start_script,
                new_shell3,
                headless,
            } = request?;
            let app = super::catalog::resolve(&catalog, &selector)
                .ok_or_else(|| format!("not in catalog: {selector}"))?;
            let path = super::bundle::download(&catalog, app, install_only).await?;
            if install_only {
                report(&format!("installed {}", app.archive_name));
                return Ok::<(), String>(());
            }
            // Backpressure preserves requests when the host launch worker pool is full.
            for _ in 0..1800 {
                let destination = if headless {
                    trueos::vshell::LaunchDestination::Headless
                } else if new_shell3 {
                    trueos::vshell::LaunchDestination::NewShell3
                } else {
                    trueos::vshell::LaunchDestination::CurrentShell
                };
                match trueos::vshell::launch_with_destination(&path, &start_script, destination) {
                    Ok(()) => {
                        report(&format!("queued {}", app.archive_name));
                        return Ok::<(), String>(());
                    }
                    Err(-16) => trueos::time::sleep(core::time::Duration::from_millis(25)).await,
                    Err(code) => {
                        return Err(format!("{}: launch enqueue failed ({code})", app.name));
                    }
                }
            }
            Err("launch queue remained busy for 45 seconds".into())
        }
        .await;
        match result {
            Ok(()) => queued += 1,
            Err(error) => {
                failed += 1;
                super::error_output(&error);
                report(&error);
            }
        }
    }
    report(&format!("drained: queued={queued} failed={failed}"));
}
