// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Platform abstractions.

#[cfg(unix)]
mod unix;

pub use std::fs::canonicalize;

#[cfg(unix)]
pub use unix::*;
