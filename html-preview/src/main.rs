use std::net::Ipv4Addr;
use trueos_ide::html_preview::{Config, PROTOCOL, Ready};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), String> {
    #[cfg(target_os = "trueos")]
    let script = trueos::async_fs::read_file_utf8(b"vFile:launch")
        .await
        .map_err(|e| format!("Cannot read preview startup script ({e})"))?;
    #[cfg(not(target_os = "trueos"))]
    let script = tokio::fs::read_to_string(
        std::env::args()
            .nth(1)
            .ok_or("Usage: html-preview startup-script")?,
    )
    .await
    .map_err(|e| e.to_string())?;
    let config = Config::parse(&script)?;
    html_preview::storage::read_asset(
        &config.root,
        &std::path::PathBuf::from(&config.root).join(&config.entry),
    )
    .await
    .map_err(|status| format!("Preview entry is unavailable ({status})"))?;
    // Bind before publishing readiness; each workspace VM owns its own port.
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let entry = config
        .entry
        .split('/')
        .map(|part| {
            percent_encoding::utf8_percent_encode(part, percent_encoding::NON_ALPHANUMERIC)
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("/");
    let ready = Ready {
        protocol: PROTOCOL.into(),
        root: config.root.clone(),
        url: format!("http://127.0.0.1:{port}/{entry}"),
    };
    let bytes = serde_json::to_vec(&ready).map_err(|e| e.to_string())?;
    html_preview::storage::write(&config.ready, &bytes).await?;
    eprintln!("html-preview: READY {}", ready.url);
    axum::serve(listener, html_preview::router(config))
        .await
        .map_err(|e| e.to_string())
}
