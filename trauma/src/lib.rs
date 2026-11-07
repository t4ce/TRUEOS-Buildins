//! Trauma is crate aiming at providing a simple way to download files
//! asynchronously via HTTP(S).

#![cfg_attr(target_os = "trueos", no_std)]

extern crate alloc;

#[macro_use]
mod output;

#[cfg(any(target_os = "trueos", target_os = "zkvm", test))]
mod association;
mod body;
mod checkpoint;
mod storage;
mod transfer;
mod transport;

pub mod download;
pub mod downloader;
pub mod path;
pub mod worker;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;

use alloc::string::String;
use core::fmt;
#[cfg(not(target_os = "trueos"))]
use std::io;
use transport::header::{ACCEPT_RANGES, CONTENT_LENGTH};

/// Errors that can happen when using Trauma.
#[derive(Debug)]
pub enum Error {
    /// Error from an underlying system.
    Internal(String),
    /// Error from the underlying URL parser or the expected URL format.
    InvalidUrl(String),
    /// I/O Error.
    #[cfg(not(target_os = "trueos"))]
    IOError { source: io::Error },
    /// Error from the Reqwest library.
    #[cfg(not(target_os = "trueos"))]
    Reqwest { source: reqwest::Error },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Internal(message) => write!(f, "Internal error: {message}"),
            Self::InvalidUrl(message) => write!(f, "Invalid URL: {message}"),
            #[cfg(not(target_os = "trueos"))]
            Self::IOError { .. } => f.write_str("I/O error"),
            #[cfg(not(target_os = "trueos"))]
            Self::Reqwest { .. } => f.write_str("Reqwest Error"),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            #[cfg(not(target_os = "trueos"))]
            Self::IOError { source } => Some(source),
            #[cfg(not(target_os = "trueos"))]
            Self::Reqwest { source } => Some(source),
            _ => None,
        }
    }
}

#[cfg(not(target_os = "trueos"))]
impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Self::IOError { source }
    }
}

#[cfg(not(target_os = "trueos"))]
impl From<reqwest::Error> for Error {
    fn from(source: reqwest::Error) -> Self {
        Self::Reqwest { source }
    }
}

/// Extension trait for `reqwest::header::HeaderMap`
///
/// Provide convenient methods for accessing common headers values.
pub(crate) trait ResponseExt {
    /// Get the content length from the headers.
    ///
    /// Returns None if the "content-length" header is missing or if its value
    /// is not a u64.
    fn content_length_header(&self) -> Option<u64>;

    /// Check whether the server supports range requests.
    ///
    /// Returns true only for the supported `bytes` range unit.
    fn accept_ranges(&self) -> bool;

    /// Get the content disposition header from the headers.
    ///
    /// Returns None if the "content-disposition" header is missing or if its
    /// value cannot be parsed as a string.
    fn content_disposition(&self) -> Option<String>;
}

impl ResponseExt for transport::Response {
    fn content_length_header(&self) -> Option<u64> {
        self.headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
    }

    fn accept_ranges(&self) -> bool {
        self.headers().get_all(ACCEPT_RANGES).iter().count() == 1
            && self
                .headers()
                .get(ACCEPT_RANGES)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.eq_ignore_ascii_case("bytes"))
    }

    fn content_disposition(&self) -> Option<String> {
        self.headers()
            .get(transport::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    }
}
