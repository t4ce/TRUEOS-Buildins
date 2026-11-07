#![cfg_attr(feature = "trueos-native", no_std)]
extern crate alloc;
#[cfg(not(feature = "trueos-native"))]
extern crate core;
#[cfg(feature = "trueos-native")]
extern crate trueos_io as io;
#[cfg(not(feature = "trueos-native"))]
use std::io;

pub use encode::*;
pub use decode::*;

mod encode;
mod decode;
mod entities;
mod io_support;

