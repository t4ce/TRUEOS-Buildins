//! Native single-hop requests with app-owned redirects and request retries.
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::time::Duration;
use futures_util::stream::{self, Stream};
use http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use trueos::netfs::http::{self as native, BodyRead, Request};
use url::Url;

const REDIRECT_LIMIT: usize = 10;
const BODY_CHUNK: usize = 32 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(2);

#[derive(Clone, Debug)]
pub struct Client {
    headers: HeaderMap,
    retries: u32,
}

impl Client {
    pub fn new(mut headers: HeaderMap, retries: u32) -> Self {
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
        Self { headers, retries }
    }

    pub async fn send(
        &self,
        method: Method,
        url: Url,
        extra: HeaderMap,
    ) -> Result<Response, String> {
        let mut headers = self.headers.clone();
        for name in extra.keys() {
            headers.remove(name);
            for value in extra.get_all(name) {
                headers.append(name.clone(), value.clone());
            }
        }
        let mut attempt = 0;
        loop {
            let result = exchange(method.clone(), url.clone(), headers.clone()).await;
            let retry = match &result {
                Ok(response) => {
                    matches!(response.status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504)
                }
                Err(error) => error.retryable,
            };
            if !retry || attempt >= self.retries {
                return result.map_err(|error| error.message);
            }
            drop(result); // Retire the old transport before retry admission.
            tokio::time::sleep(Duration::from_secs(1u64 << attempt.min(5))).await;
            attempt += 1;
        }
    }
}

pub struct Response {
    status: StatusCode,
    url: Url,
    headers: HeaderMap,
    request: Option<Request>,
}

impl Response {
    pub fn status(&self) -> StatusCode {
        self.status
    }
    pub fn url(&self) -> &Url {
        &self.url
    }
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }
    pub fn bytes_stream(self) -> impl Stream<Item = Result<Vec<u8>, String>> {
        stream::unfold(self.request, |state| async move {
            let mut request = state?;
            let mut chunk = Vec::new();
            if chunk.try_reserve_exact(BODY_CHUNK).is_err() {
                return Some((Err("cannot allocate HTTP receive buffer".into()), None));
            }
            chunk.resize(BODY_CHUNK, 0);
            loop {
                match request.try_read(&mut chunk) {
                    Ok(BodyRead::Pending) => tokio::time::sleep(POLL_INTERVAL).await,
                    Ok(BodyRead::Data(count)) => {
                        chunk.truncate(count);
                        return Some((Ok(chunk), Some(request)));
                    }
                    Ok(BodyRead::Eof) => return None,
                    Err(code) => return Some((Err(format!("HTTP body failed ({code})")), None)),
                }
            }
        })
    }
}

// Only errors known to be transient before the response head may consume the
// request retry budget. TLS/protocol/policy errors remain explicit failures;
// body failures are never replayed after bytes have been accepted.
struct Failure {
    message: String,
    retryable: bool,
}
impl Failure {
    fn native(context: &str, code: i32) -> Self {
        Self {
            message: format!("{context} failed ({code})"),
            retryable: matches!(code, -12 | -11 | -111 | -112 | -113 | -114),
        }
    }
}
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            retryable: false,
        }
    }
}
impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        String::from(message).into()
    }
}

async fn exchange(
    method: Method,
    mut url: Url,
    mut headers: HeaderMap,
) -> Result<Response, Failure> {
    let is_head = method == Method::HEAD;
    let method = if is_head {
        native::Method::Head
    } else {
        native::Method::Get
    };
    for hop in 0..=REDIRECT_LIMIT {
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err("redirect is not an HTTP(S) URL".into());
        }
        apply_userinfo(&mut url, &mut headers)?;
        url.set_fragment(None);
        let spec = native::RequestOwned {
            method,
            url: url.as_str().into(),
            headers: headers
                .iter()
                .map(|(name, value)| native::Header {
                    name: name.as_str().into(),
                    value: value.as_bytes().into(),
                })
                .collect(),
            timeouts: native::Timeouts {
                connect_ms: 0,
                tls_ms: 0,
                read_idle_ms: 0,
            },
        };
        // Canceled transports retain their admission slot until the native
        // cleanup fence returns. Wait cooperatively when32 workers redirect
        // together; admission pressure is not a request retry or HTTP error.
        let mut request = loop {
            match Request::start(&spec) {
                Ok(request) => break request,
                Err(-3) => tokio::time::sleep(POLL_INTERVAL).await,
                Err(code) => return Err(Failure::native("HTTP start", code)),
            }
        };
        let head = loop {
            match request
                .try_head()
                .map_err(|code| Failure::native("HTTP response", code))?
            {
                Some(head) => break head,
                None => tokio::time::sleep(POLL_INTERVAL).await,
            }
        };
        let status =
            StatusCode::from_u16(head.status).map_err(|_| "invalid HTTP response status")?;
        let response_headers = parse_headers(head.headers)?;
        if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
            if response_headers.get_all(header::LOCATION).iter().count() > 1 {
                return Err("multiple redirect Location fields".into());
            }
            if let Some(location) = response_headers.get(header::LOCATION) {
                if hop == REDIRECT_LIMIT {
                    return Err("HTTP redirect limit exceeded".into());
                }
                let location = location
                    .to_str()
                    .map_err(|_| "redirect Location is not UTF8")?;
                let next = url
                    .join(location)
                    .map_err(|_| "invalid redirect Location")?;
                if origin_changed(&url, &next) {
                    for name in [
                        header::AUTHORIZATION,
                        header::COOKIE,
                        header::PROXY_AUTHORIZATION,
                        header::WWW_AUTHENTICATE,
                    ] {
                        headers.remove(name);
                    }
                    headers.remove("cookie2");
                }
                // Preserve the hosted client's redirect Referer behavior using
                // this hop's already sanitized URL. Do not leak it on downgrade.
                if url.scheme() == "https" && next.scheme() == "http" {
                    headers.remove(header::REFERER);
                } else if let Ok(referer) = HeaderValue::from_str(url.as_str()) {
                    headers.insert(header::REFERER, referer);
                }
                drop(request);
                url = next;
                continue;
            }
        }
        // The transport is single-hop. Keep the app's canonical, sanitized URL
        // as the final representation identity rather than trusting a rewrite.
        // The owned head has no unread body. Release its handle immediately:
        // retaining 32 completed HEAD handles while starting 32 GETs would
        // unnecessarily double worker transport admission and memory.
        let request = if is_head {
            drop(request);
            None
        } else {
            Some(request)
        };
        return Ok(Response {
            status,
            url,
            headers: response_headers,
            request,
        });
    }
    Err("HTTP redirect limit exceeded".into())
}

fn parse_headers(values: Vec<native::Header>) -> Result<HeaderMap, String> {
    let mut result = HeaderMap::new();
    for value in values {
        let name = header::HeaderName::from_bytes(value.name.as_bytes())
            .map_err(|_| "invalid response header name")?;
        let value =
            HeaderValue::from_bytes(&value.value).map_err(|_| "invalid response header value")?;
        result.append(name, value);
    }
    Ok(result)
}

fn origin_changed(old: &Url, next: &Url) -> bool {
    old.scheme() != next.scheme()
        || old.host_str() != next.host_str()
        || old.port_or_known_default() != next.port_or_known_default()
}

fn apply_userinfo(url: &mut Url, headers: &mut HeaderMap) -> Result<(), String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    if !url.username().is_empty() || url.password().is_some() {
        let mut credentials: Vec<u8> =
            percent_encoding::percent_decode_str(url.username()).collect();
        credentials.push(b':');
        if let Some(password) = url.password() {
            credentials.extend(percent_encoding::percent_decode_str(password));
        }
        let authorization = format!("Basic {}", STANDARD.encode(credentials));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&authorization).map_err(|_| "invalid URL credentials")?,
        );
        url.set_username("").map_err(|_| "invalid URL authority")?;
        url.set_password(None)
            .map_err(|_| "invalid URL authority")?;
    }
    Ok(())
}
