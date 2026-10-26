//! # Core types for Winit
//!
//! Platform-agnostic types and traits useful when implementing Winit backends,
//! or otherwise interfacing with Winit from library code.
//!
//! See the [`winit`] crate for the full user-facing API.
//!
//! [`winit`]: https://docs.rs/winit

// Every newly exported enum should either be `#[non_exhaustive]`, or carry an `#[allow]` of
// this lint when the set of variants can never grow.
// `clippy::exhaustive_structs` is deliberately not enabled: event structs must stay
// constructible by the backend crates, which `#[non_exhaustive]` would forbid.
#![warn(clippy::exhaustive_enums)]

#[macro_use]
pub mod casting;
pub mod cursor;
#[macro_use]
pub mod error;
pub mod application;
pub mod data_transfer;
pub mod event;
pub mod event_loop;
pub mod icon;
pub mod keyboard;
pub mod monitor;
pub mod window;

pub(crate) use std::time::Instant;
