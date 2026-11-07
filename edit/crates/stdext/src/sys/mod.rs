// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Platform abstractions.

#[allow(unused_imports)]
use crate::platform::prelude::*;

#[cfg(any(unix, target_os = "trueos"))]
mod unix;

pub use crate::platform::fs::canonicalize;

#[cfg(any(unix, target_os = "trueos"))]
pub use unix::*;
