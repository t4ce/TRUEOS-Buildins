use core::fmt::Display;

use http::HeaderValue;

pub(crate) fn fmt<T: Display>(fmt: T) -> HeaderValue {
    let s = fmt.to_string();
    match HeaderValue::from_maybe_shared(s) {
        Ok(val) => val,
        Err(err) => panic!("illegal HeaderValue; error = {:?}, fmt = \"{}\"", err, fmt),
    }
}

#[cfg(target_os = "trueos")]
#[allow(unused_imports)]
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec, borrow::ToOwned};
