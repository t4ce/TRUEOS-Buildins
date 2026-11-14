//! Restore fetched state from disk, or the live RAM catalog without a filesystem.
use alloc::{collections::BTreeSet, format, string::String};
use trueos::{async_fs, content_identity::ContentTypeId};

pub async fn load() -> Result<BTreeSet<String>, String> {
    let mounts = async_fs::list_mounts().await
        .map_err(|error| format!("Read filesystem availability: {error}"))?;
    if mounts.is_empty() {
        let bytes = async_fs::read_file(b"vFile:appdb").await
            .map_err(|error| format!("Read RAM AppDB: {error}"))?;
        let catalog: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Read RAM AppDB: {error}"))?;
        let apps = catalog.get("apps").and_then(|apps| apps.as_array())
            .ok_or_else(|| String::from("Invalid RAM AppDB catalog"))?;
        return apps.iter().map(|app| {
            app.get("archive").and_then(|name| name.as_str()).map(String::from)
                .ok_or_else(|| String::from("Invalid RAM AppDB archive"))
        }).collect();
    }
    let listing = match async_fs::list_dir_typed(super::download::DIRECTORY.as_bytes()).await {
        Ok(listing) => listing,
        Err(async_fs::ERR_NOT_FOUND) => return Ok(BTreeSet::new()),
        Err(error) => return Err(format!("Read fetched apps: {error}")),
    };
    if listing.truncated { return Err("Fetched app listing is incomplete".into()); }
    Ok(listing.entries.into_iter().filter_map(|entry| {
        (entry.kind == async_fs::NodeKind::File && entry.content_type == ContentTypeId::TRUEOS_BLUEPRINT)
            .then_some(entry.name)
    }).collect())
}
