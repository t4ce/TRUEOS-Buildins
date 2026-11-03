//! A small native starting point alongside a complete pinned core-js reference.
//! JS object/value coercion and engine bindings belong to the embedding host.
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

pub mod abstract_ops;
pub mod string;

pub const UPSTREAM_VERSION: &str = "3.51.0";
