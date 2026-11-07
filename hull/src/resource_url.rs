//! Public pages use HTTPS; local IDE preview Blueprints use loopback HTTP.
use alloc::{string::{String, ToString}};
pub(crate) fn allowed(url: &url::Url) -> bool {
    url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some()
        && (url.scheme() == "https"
            || (url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.port().is_some_and(|port| port != 0)))
}

/// The document base comes from the response that supplied its bytes. Reject
/// an unavailable or disallowed transport URL instead of using an old origin.
pub(crate) fn document_response_url(response_url: &str) -> Result<String, i32> {
    let url = url::Url::parse(response_url).map_err(|_| -1)?;
    if !allowed(&url) { return Err(-1); }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn redirected_document_uses_the_response_host_and_directory_for_assets() {
        let response = super::document_response_url("https://cdn.test:443/new/directory/index.html?variant=1").unwrap();
        assert_eq!(response, "https://cdn.test/new/directory/index.html?variant=1");
        let base = url::Url::parse(&response).unwrap();
        assert_eq!(base.join("logo.png").unwrap().as_str(), "https://cdn.test/new/directory/logo.png");
        assert_eq!(base.join("../style/site.css").unwrap().as_str(), "https://cdn.test/new/style/site.css");
        assert_eq!(base.join("/scripts/main.js").unwrap().as_str(), "https://cdn.test/scripts/main.js");
        for bad in ["", "relative/index.html", "https://user:password@cdn.test/index.html", "http://cdn.test/index.html", "file:///tmp/page.html"] {
            assert!(super::document_response_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn preview_is_loopback_only_and_requires_an_explicit_port() {
        for u in ["https://example.com/a", "http://127.0.0.1:49180/index.html"] {
            assert!(super::allowed(&url::Url::parse(u).unwrap()));
        }
        for u in [
            "http://example.com:49180/",
            "http://127.0.0.1/",
            "http://127.0.0.1:0/",
            "file:///tmp/index.html",
            "https://user@example.com/",
        ] {
            assert!(!super::allowed(&url::Url::parse(u).unwrap()), "{u}");
        }
    }
}
