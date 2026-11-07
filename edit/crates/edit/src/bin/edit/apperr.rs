// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[allow(unused_imports)]
use stdext::platform::prelude::*;

use stdext::platform::io;

use edit::{buffer, icu};

#[derive(Debug)]
pub enum Error {
    SettingsInvalid(&'static str),
    Io(io::Error),
    Icu(icu::Error),
}

pub type Result<T> = stdext::platform::result::Result<T, Error>;

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<icu::Error> for Error {
    fn from(err: icu::Error) -> Self {
        Self::Icu(err)
    }
}

impl From<buffer::IoError> for Error {
    fn from(err: buffer::IoError) -> Self {
        match err {
            buffer::IoError::Io(e) => Self::Io(e),
            buffer::IoError::Icu(e) => Self::Icu(e),
        }
    }
}
