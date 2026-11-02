//! Native translations of the portable parts of the vendored jQuery source.
//! Browser operations are explicit host contracts, not emulated browser state.
#![doc = include_str!("../README.md")]
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod ajax;
pub mod callbacks;
pub mod collection;
pub mod data;
pub mod deferred;
pub mod dom;
pub mod effects;
pub mod events;
pub mod queue;
pub mod serialize;
pub mod style;
pub mod utilities;
pub mod value;

pub use collection::{JQuery, Selection};
pub use dom::{Dom, Error, NodeKind};
pub use value::Value;

/// Source version, not a claim that every browser-dependent API is implemented.
pub const UPSTREAM_VERSION: &str = "4.0.0";
