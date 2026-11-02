//! Trauma is crate aiming at providing a simple way to download files
//! asynchronously via HTTP(S).

extern crate alloc;

pub mod download;
pub mod downloader;
pub mod path;

use alloc::string::String;
use core::fmt;
use reqwest::header::{ACCEPT_RANGES, CONTENT_LENGTH};
use std::io;

/// Errors that can happen when using Trauma.
#[derive(Debug)]
pub enum Error {
    /// Error from an underlying system.
    Internal(String),
    /// Error from the underlying URL parser or the expected URL format.
    InvalidUrl(String),
    /// I/O Error.
    IOError { source: io::Error },
    /// Error from the Reqwest library.
    Reqwest { source: reqwest::Error },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Internal(message) => write!(f, "Internal error: {message}"),
            Self::InvalidUrl(message) => write!(f, "Invalid URL: {message}"),
            Self::IOError { .. } => f.write_str("I/O error"),
            Self::Reqwest { .. } => f.write_str("Reqwest Error"),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::IOError { source } => Some(source),
            Self::Reqwest { source } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Self::IOError { source }
    }
}

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
    /// Returns false if the "accept-ranges" header is missing or if its value
    /// is "none".
    fn accept_ranges(&self) -> bool;

    /// Get the content disposition header from the headers.
    ///
    /// Returns None if the "content-disposition" header is missing or if its
    /// value cannot be parsed as a string.
    fn content_disposition(&self) -> Option<String>;
}

impl ResponseExt for reqwest::Response {
    fn content_length_header(&self) -> Option<u64> {
        self.headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
    }

    fn accept_ranges(&self) -> bool {
        match self.headers().get(ACCEPT_RANGES) {
            None => false,
            Some(x) if x == "none" => false,
            Some(_) => true,
        }
    }

    fn content_disposition(&self) -> Option<String> {
        self.headers()
            .get(reqwest::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    }
}
