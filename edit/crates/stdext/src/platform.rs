//! Rust data types and editor services, with a native TRUEOS implementation.
//! Host tools retain their ordinary standard library implementation.

#[cfg(not(target_os = "trueos"))]
pub use std::*;

#[cfg(target_os = "trueos")]
pub use core::*;
#[cfg(target_os = "trueos")]
pub use crate::rust_alloc::{borrow, boxed, collections, format, rc, string, vec};

#[cfg(not(target_os = "trueos"))]
pub mod prelude {
    pub use std::prelude::rust_2024::*;
}
#[cfg(target_os = "trueos")]
pub mod prelude {
    pub use crate::rust_alloc::{borrow::ToOwned, boxed::Box, format, string::{String, ToString}, vec, vec::Vec};
    pub use super::path::{NativeName, NativeOwnedName, NativeOwnedPath, NativePath};
}

#[cfg(target_os = "trueos")]
pub mod alloc {
    pub use crate::rust_alloc::alloc::*;
}

#[cfg(target_os = "trueos")]
pub mod io {
    pub use trueos_io::{Read, Write, BufRead, Cursor, Seek, SeekFrom, ErrorKind};
    pub use trueos_io::{Error as StreamError, Result as StreamResult};
    use super::{prelude::*, fmt};

    /// Application diagnostics retain owned compiler/service messages. Stream
    /// traits themselves remain the shared TRUEOS I/O traits and error type.
    #[derive(Debug)]
    pub struct Error { kind: ErrorKind, message: String }
    pub type Result<T> = core::result::Result<T, Error>;
    impl Error {
        pub fn new(kind: ErrorKind, message: impl fmt::Display) -> Self {
            Self { kind, message: message.to_string() }
        }
        pub fn other(message: impl fmt::Display) -> Self { Self::new(ErrorKind::Other, message) }
        pub fn kind(&self) -> ErrorKind { self.kind }
    }
    impl From<StreamError> for Error {
        fn from(error: StreamError) -> Self { Self::new(error.kind(), error) }
    }
    impl From<ErrorKind> for Error {
        fn from(kind: ErrorKind) -> Self { Self::new(kind, "editor I/O operation failed") }
    }
    impl fmt::Display for Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.message) }
    }
    impl core::error::Error for Error {}
}
#[cfg(target_os = "trueos")]
pub mod time {
    pub use core::time::Duration;
    pub use trueos_runtime::time::Instant;
}

#[cfg(target_os = "trueos")]
pub mod ffi {
    pub use core::ffi::*;
    pub type OsStr = str;
    pub type OsString = crate::rust_alloc::string::String;
}

#[cfg(target_os = "trueos")]
pub mod env {
    use super::{io, path::PathBuf};
    pub use trueos::env::{args, var, VarError};
    pub use trueos::env::args as args_os;
    pub fn var_os(key: &str) -> Option<super::string::String> { var(key).ok() }
    pub fn current_dir() -> io::Result<PathBuf> {
        trueos::env::current_dir().map(PathBuf::from)
            .map_err(|_| io::Error::other("native launch directory unavailable"))
    }
}

#[cfg(target_os = "trueos")]
pub mod path {
    use super::{borrow::Cow, string::String, vec::Vec};
    pub use trueos_path::{Component, Components, Path, PathBuf};
    pub const MAIN_SEPARATOR_STR: &str = "/";
    pub fn is_separator(value: char) -> bool { value == '/' }

    // These conveniences expose the native UTF-8 path as Rust text. They do not
    // introduce a host OS string encoding or filesystem descriptor interface.
    pub trait NativePath {
        fn as_os_str(&self) -> &str;
        fn to_str(&self) -> Option<&str>;
        fn to_string_lossy(&self) -> Cow<'_, str>;
        fn exists(&self) -> bool;
        fn is_dir(&self) -> bool;
    }
    impl NativePath for Path {
        fn as_os_str(&self) -> &str { self.as_str() }
        fn to_str(&self) -> Option<&str> { Some(self.as_str()) }
        fn to_string_lossy(&self) -> Cow<'_, str> { Cow::Borrowed(self.as_str()) }
        fn exists(&self) -> bool { super::fs::metadata(self).is_ok() }
        fn is_dir(&self) -> bool { super::fs::metadata(self).is_ok_and(|m| m.is_dir()) }
    }
    pub trait NativeOwnedPath { fn into_os_string(self) -> String; }
    impl NativeOwnedPath for PathBuf {
        fn into_os_string(self) -> String { self.into_string() }
    }
    pub trait NativeName {
        fn new(value: &str) -> &Self;
        unsafe fn from_encoded_bytes_unchecked(value: &[u8]) -> &Self;
        fn as_encoded_bytes(&self) -> &[u8];
        fn to_string_lossy(&self) -> Cow<'_, str>;
    }
    impl NativeName for str {
        fn new(value: &str) -> &Self { value }
        unsafe fn from_encoded_bytes_unchecked(value: &[u8]) -> &Self {
            unsafe { core::str::from_utf8_unchecked(value) }
        }
        fn as_encoded_bytes(&self) -> &[u8] { self.as_bytes() }
        fn to_string_lossy(&self) -> Cow<'_, str> { Cow::Borrowed(self) }
    }
    pub trait NativeOwnedName {
        fn into_encoded_bytes(self) -> Vec<u8>;
        unsafe fn from_encoded_bytes_unchecked(value: Vec<u8>) -> Self;
    }
    impl NativeOwnedName for String {
        fn into_encoded_bytes(self) -> Vec<u8> { self.into_bytes() }
        unsafe fn from_encoded_bytes_unchecked(value: Vec<u8>) -> Self {
            unsafe { String::from_utf8_unchecked(value) }
        }
    }
}

#[cfg(target_os = "trueos")]
#[path = "native_fs.rs"]
pub mod fs;

/// Keep the editor's AVX2 paths behind both CPU and enabled XSAVE state checks.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn avx2_available() -> bool {
    #[cfg(not(target_os = "trueos"))]
    { std::is_x86_feature_detected!("avx2") }
    #[cfg(target_os = "trueos")]
    unsafe {
        #[cfg(target_arch = "x86")]
        use core::arch::x86::{__cpuid, __cpuid_count, _xgetbv};
        #[cfg(target_arch = "x86_64")]
        use core::arch::x86_64::{__cpuid, __cpuid_count, _xgetbv};
        if __cpuid(0).eax < 7 { return false; }
        let features = __cpuid(1).ecx;
        let required = (1 << 26) | (1 << 27) | (1 << 28);
        features & required == required && _xgetbv(0) & 6 == 6
            && __cpuid_count(7, 0).ebx & (1 << 5) != 0
    }
}

pub fn powi(value: f64, exponent: i32) -> f64 {
    #[cfg(not(target_os = "trueos"))]
    { value.powi(exponent) }
    #[cfg(target_os = "trueos")]
    {
        let mut factor = if exponent < 0 { 1.0 / value } else { value };
        let mut power = exponent.unsigned_abs();
        let mut result = 1.0;
        while power != 0 {
            if power & 1 != 0 { result *= factor; }
            power >>= 1;
            if power != 0 { factor *= factor; }
        }
        result
    }
}

pub fn powf(value: f32, exponent: f32) -> f32 {
    #[cfg(not(target_os = "trueos"))]
    { value.powf(exponent) }
    #[cfg(target_os = "trueos")]
    unsafe {
        unsafe extern "C" { fn powf(value: f32, exponent: f32) -> f32; }
        powf(value, exponent)
    }
}
