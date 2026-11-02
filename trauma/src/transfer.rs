//! Validate the wire response before choosing any destructive filesystem operation.
use crate::{
    checkpoint::{fingerprint, strong_etag, Checkpoint},
    ResponseExt,
};
use alloc::{
    format,
    string::{String, ToString},
};
use reqwest::{
    header::{HeaderMap, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE},
    Response, StatusCode,
};

#[derive(Debug)]
pub(crate) struct TransferPlan {
    pub offset: u64,
    pub total: Option<u64>,
}

pub(crate) fn identity_encoding(headers: &HeaderMap) -> Result<(), String> {
    for value in headers.get_all(CONTENT_ENCODING) {
        if !value
            .to_str()
            .is_ok_and(|v| v.eq_ignore_ascii_case("identity"))
        {
            return Err(
                "non-identity Content-Encoding is incompatible with saved byte offsets".to_string(),
            );
        }
    }
    Ok(())
}

pub(crate) fn content_length(headers: &HeaderMap) -> Result<Option<u64>, String> {
    if headers.get_all(CONTENT_LENGTH).iter().count() > 1 {
        return Err("multiple Content-Length values".to_string());
    }
    headers
        .get(CONTENT_LENGTH)
        .map(|v| {
            let text = v
                .to_str()
                .map_err(|_| "invalid Content-Length".to_string())?;
            number(text).ok_or_else(|| "invalid Content-Length".to_string())
        })
        .transpose()
}

fn number(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn range(headers: &HeaderMap) -> Result<(u64, u64, u64), String> {
    let invalid = || "invalid or missing Content-Range".to_string();
    if headers.get_all(CONTENT_RANGE).iter().count() != 1 {
        return Err(invalid());
    }
    let text = headers
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(invalid)?;
    let (unit, value) = text.split_once(' ').ok_or_else(invalid)?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return Err(invalid());
    }
    let (span, total) = value.split_once('/').ok_or_else(invalid)?;
    let (start, end) = span.split_once('-').ok_or_else(invalid)?;
    let (start, end, total) = (
        number(start).ok_or_else(invalid)?,
        number(end).ok_or_else(invalid)?,
        number(total).ok_or_else(invalid)?,
    );
    if start > end || end >= total {
        return Err(invalid());
    }
    Ok((start, end, total))
}

impl TransferPlan {
    pub fn validate(
        response: &Response,
        resume: Option<(&Checkpoint, u64)>,
    ) -> Result<Self, String> {
        identity_encoding(response.headers())?;
        if response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case("multipart/byteranges")
            })
        {
            return Err("multipart byte ranges are unsupported".to_string());
        }
        let length = content_length(response.headers())?;
        match response.status() {
            StatusCode::OK => {
                if response.headers().contains_key(CONTENT_RANGE) {
                    return Err("unexpected Content-Range on a full response".to_string());
                }
                Ok(Self {
                    offset: 0,
                    total: length,
                })
            }
            StatusCode::PARTIAL_CONTENT => {
                let (checkpoint, offset) =
                    resume.ok_or_else(|| "unsolicited partial response".to_string())?;
                let (start, end, total) = range(response.headers())?;
                if start != offset
                    || end != total - 1
                    || length.is_some_and(|n| n != end - start + 1)
                {
                    return Err("Content-Range does not match the requested suffix".to_string());
                }
                if checkpoint.total.is_some_and(|n| n != total)
                    || checkpoint.resolved != fingerprint(response.url())
                    || strong_etag(response.headers()).as_deref() != Some(checkpoint.etag.as_str())
                {
                    return Err("partial response changed representation identity".to_string());
                }
                Ok(Self {
                    offset,
                    total: Some(total),
                })
            }
            status => Err(format!("unexpected GET status {status}")),
        }
    }
}

pub(crate) fn can_resume(
    head: &Response,
    checkpoint: &Checkpoint,
    size: u64,
    requested: &reqwest::Url,
) -> bool {
    !checkpoint.complete
        && size > 0
        && checkpoint.total.is_none_or(|n| size < n)
        && checkpoint.matches(
            requested,
            head.url(),
            strong_etag(head.headers()).as_deref(),
        )
        && crate::download::Download::is_resumable(head)
        && identity_encoding(head.headers()).is_ok()
        && checkpoint
            .total
            .is_none_or(|n| head.content_length_header() == Some(n))
}
