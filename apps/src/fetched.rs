//! Restore fetched state from the archives already durably published on disk.
use alloc::{collections::BTreeSet, format, string::String};
use trueos::{async_fs, content_identity::ContentTypeId};

pub async fn load() -> Result<BTreeSet<String>, String> {
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
