// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Platform abstractions.

#[allow(unused_imports)]
use stdext::platform::prelude::*;

#[cfg(all(unix, not(target_os = "trueos")))]
mod unix;

#[cfg(not(target_os = "trueos"))]
pub use stdext::platform::fs::canonicalize;

#[cfg(all(unix, not(target_os = "trueos")))]
pub use unix::*;

#[cfg(target_os = "trueos")]
mod native;
#[cfg(target_os = "trueos")]
pub use native::*;

#[cfg(target_os = "trueos")]
mod trueos_terminal;
#[cfg(target_os = "trueos")]
pub use trueos_terminal::*;

#[cfg(target_os = "trueos")]
mod trueos_output;
#[cfg(target_os = "trueos")]
pub use trueos_output::write_stdout;
