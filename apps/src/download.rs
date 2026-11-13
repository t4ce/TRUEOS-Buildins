//! A download becomes visible only after verification and durable publication.
use alloc::{format, string::String};

pub const DIRECTORY: &str = "common/dl/apps";

pub async fn download(app: &super::catalog::OnlineApp) -> Result<String, String> {
    let bytes = super::fetch::bytes(super::catalog::url(app), super::fetch::ARCHIVE_LIMIT).await?;
    let verify_started = trueos::time::Instant::now();
    if !super::catalog::verify(app, &bytes) {
        return Err("Blueprint SHA-256 mismatch".into());
    }
    super::pullbot::report(&format!(
        "download phase=verify elapsed_ms={} bytes={}",
        verify_started.elapsed().as_millis(),
        bytes.len()
    ));
    let save_started = trueos::time::Instant::now();
    let dir = DIRECTORY;
    trueos::async_fs::create_dir_all(dir.as_bytes())
        .await
        .map_err(|e| format!("Create download directory: {e}"))?;
    let path = format!("{dir}/{}", app.archive_name);
    trueos::async_fs::write_file_typed_checked(
        path.as_bytes(),
        &bytes,
        trueos::content_identity::ContentTypeId::TRUEOS_BLUEPRINT,
        false,
        None,
    )
    .await
    .map_err(|e| format!("Save download: {e}"))?;
    super::pullbot::report(&format!(
        "download phase=save elapsed_ms={} bytes={}",
        save_started.elapsed().as_millis(),
        bytes.len()
    ));
    Ok(path)
}
