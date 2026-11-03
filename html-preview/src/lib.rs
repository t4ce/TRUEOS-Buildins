//! A read-only folder server. Edit owns publication; this VM owns HTTP only.
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::Response,
    routing::get,
};
use std::{path::PathBuf, sync::Arc};
use trueos_ide::html_preview::{Config, valid_asset_path};

pub const MAX_ASSET_BYTES: u64 = 8 * 1024 * 1024;

pub fn router(config: Config) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/{*path}", get(asset))
        .with_state(Arc::new(config))
}

async fn index(State(config): State<Arc<Config>>) -> Response {
    serve(&config, &config.entry).await
}
async fn asset(State(config): State<Arc<Config>>, Path(path): Path<String>) -> Response {
    serve(&config, &path).await
}

async fn serve(config: &Config, path: &str) -> Response {
    if !valid_asset_path(path) || path.split('/').any(|p| p.ends_with(".preview-tmp")) {
        return reply(
            StatusCode::BAD_REQUEST,
            "text/plain",
            b"Invalid asset path".to_vec(),
        );
    }
    let path = PathBuf::from(&config.root).join(path);
    match storage::read_asset(&config.root, &path).await {
        Ok(bytes) => reply(
            StatusCode::OK,
            content_type(path.extension().and_then(|e| e.to_str()).unwrap_or("")),
            bytes,
        ),
        Err(code) => reply(code, "text/plain", format!("{code}").into_bytes()),
    }
}

fn reply(status: StatusCode, mime: &str, bytes: Vec<u8>) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(bytes))
        .unwrap()
}

fn content_type(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

pub mod storage {
    use super::*;
    #[cfg(not(target_os = "trueos"))]
    pub async fn read_asset(root: &str, path: &std::path::Path) -> Result<Vec<u8>, StatusCode> {
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
        let path = tokio::fs::canonicalize(path)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
        if !path.starts_with(&root) {
            return Err(StatusCode::FORBIDDEN);
        }
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
        if !metadata.is_file() {
            return Err(StatusCode::NOT_FOUND);
        }
        if metadata.len() > MAX_ASSET_BYTES {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if bytes.len() as u64 > MAX_ASSET_BYTES {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        Ok(bytes)
    }
    #[cfg(target_os = "trueos")]
    pub async fn read_asset(_root: &str, path: &std::path::Path) -> Result<Vec<u8>, StatusCode> {
        // TRUEOSFS has regular file/directory nodes, not host symlinks. Only
        // normalized relative asset paths reach this app-scoped document root.
        let path = path.to_str().ok_or(StatusCode::BAD_REQUEST)?.as_bytes();
        let metadata = trueos::async_fs::metadata(path)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
        if metadata.len > MAX_ASSET_BYTES {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        let bytes = trueos::async_fs::read_file(path)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
        if bytes.len() as u64 > MAX_ASSET_BYTES {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        Ok(bytes)
    }
    pub async fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
        #[cfg(target_os = "trueos")]
        return trueos::async_fs::write_file(path.as_bytes(), bytes)
            .await
            .map_err(|e| format!("TRUEOSFS write failed ({e})"));
        #[cfg(not(target_os = "trueos"))]
        tokio::fs::write(path, bytes)
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    #[tokio::test]
    async fn serves_folder_updates_types_and_rejects_escape() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "first").unwrap();
        std::fs::write(dir.path().join("style.css"), "body{}").unwrap();
        let app = router(Config {
            root: dir.path().to_str().unwrap().into(),
            ready: "/unused/ready".into(),
            entry: "index.html".into(),
        });
        let request = |path: &str| Request::builder().uri(path).body(Body::empty()).unwrap();
        let response = app.clone().oneshot(request("/")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "first"
        );
        std::fs::write(dir.path().join("index.html"), "second").unwrap();
        let response = app.clone().oneshot(request("/")).await.unwrap();
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "second"
        );
        let response = app.clone().oneshot(request("/style.css")).await.unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );
        for path in [
            "/%2e%2e/private",
            "/a%2f..%2fprivate",
            "/a%5cb",
            "/a.preview-tmp",
        ] {
            assert_eq!(
                app.clone().oneshot(request(path)).await.unwrap().status(),
                StatusCode::BAD_REQUEST,
                "{path}"
            );
        }
        assert_eq!(
            app.clone()
                .oneshot(request("/missing"))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
