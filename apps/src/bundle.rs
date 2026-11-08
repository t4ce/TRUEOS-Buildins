//! All fetch entry points use the bundle definitions embedded in AppDB.
use super::catalog::{self, OnlineApp};
use alloc::{format, string::String, vec::Vec};

/// Resolve the entire request before downloading, then publish it atomically.
/// The returned pathname always belongs to the requested app, never a helper.
pub async fn download(
    catalog: &[OnlineApp],
    app: &OnlineApp,
    install_only: bool,
) -> Result<String, String> {
    let json = trueos::async_fs::read_file(trueos_appbundle::VFILE)
        .await
        .map_err(|error| format!("{}: cannot read AppDB app bundles ({error})", app.name))?;
    let bundles = trueos_appbundle::AppBundles::parse(&json)?;
    let names = bundles.members(&app.archive_name)?;
    let members = names
        .iter()
        .map(|name| {
            catalog::resolve(catalog, name)
                .ok_or_else(|| format!("{}: required {name} unavailable in app catalog", app.name))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut paths = Vec::new();
    for member in &members {
        let path = super::download::download(member).await.map_err(|error| {
            format!("{}: {} unavailable: {error}", app.name, member.archive_name)
        })?;
        paths.push(path);
    }
    if install_only || members.len() > 1 {
        trueos::async_fs::write_file(b"vFile:appdb-install", paths.join("\n").as_bytes())
            .await
            .map_err(|error| {
                format!(
                    "{}: AppDB bundle import failed ({error}); not started",
                    app.name
                )
            })?;
    }
    paths
        .pop()
        .ok_or_else(|| format!("{}: empty app bundle", app.name))
}
