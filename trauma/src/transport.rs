//! HTTP exchange adapters. Resume, naming and storage policy stay in Trauma.
pub use http::{header, HeaderMap, Method, StatusCode};
pub use url::Url;

#[cfg(target_os = "trueos")]
#[path = "native_http.rs"]
mod native;
#[cfg(target_os = "trueos")]
pub use native::{Client, Response};
#[cfg(target_os = "trueos")]
pub type SendError = alloc::string::String;
#[cfg(target_os = "trueos")]
pub type InitError = alloc::string::String;

#[cfg(not(target_os = "trueos"))]
pub type Client = reqwest_middleware::ClientWithMiddleware;
#[cfg(not(target_os = "trueos"))]
pub type Response = reqwest::Response;
#[cfg(not(target_os = "trueos"))]
pub type SendError = reqwest_middleware::Error;
#[cfg(not(target_os = "trueos"))]
pub type InitError = reqwest::Error;

pub async fn send(
    client: &Client,
    method: Method,
    url: Url,
    headers: HeaderMap,
) -> Result<Response, SendError> {
    #[cfg(target_os = "trueos")]
    {
        client.send(method, url, headers).await
    }
    #[cfg(not(target_os = "trueos"))]
    {
        client.request(method, url).headers(headers).send().await
    }
}
