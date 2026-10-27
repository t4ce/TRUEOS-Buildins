// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Platform abstractions.

#[cfg(unix)]
mod unix;

#[cfg(not(target_os = "trueos"))]
pub use std::fs::canonicalize;

#[cfg(unix)]
pub use unix::*;

#[cfg(target_os = "trueos")]
mod trueos_abi;
