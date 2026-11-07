//! I/O vocabulary selected by the terminal client.
//!
//! `native-core` on TRUEOS uses the shared platform reader/writer/error types.
//! Other clients keep the existing standard-library I/O API.

#[cfg(not(all(target_os = "trueos", feature = "native-core")))]
pub use std::io::*;

#[cfg(all(target_os = "trueos", feature = "native-core"))]
pub use trueos_io::*;

#[cfg(all(target_os = "trueos", feature = "native-core"))]
pub use crate::trueos::Output as Stdout;
#[cfg(all(target_os = "trueos", feature = "native-core"))]
pub use crate::trueos::OutputLock as StdoutLock;

/// The attached Blueprint terminal; no descriptor discovery is involved.
#[cfg(all(target_os = "trueos", feature = "native-core"))]
pub fn stdout() -> Stdout { crate::trueos::Output }
