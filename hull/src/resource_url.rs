//! Public pages use HTTPS; local IDE preview Blueprints use loopback HTTP.
pub(crate) fn allowed(url: &url::Url) -> bool {
    url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some()
        && (url.scheme() == "https"
            || (url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.port().is_some_and(|port| port != 0)))
}

#[cfg(test)]
mod tests {
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
