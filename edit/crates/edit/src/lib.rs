#![cfg_attr(target_os = "trueos", no_std)]
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(
    target_arch = "loongarch64",
    feature(stdarch_loongarch, stdarch_loongarch_feature_detection, loongarch_target_feature),
    allow(clippy::incompatible_msrv)
)]
#![allow(clippy::missing_transmute_annotations, clippy::new_without_default, stable_features)]

#[allow(unused_imports)]
use stdext::platform::prelude::*;

pub mod base64;
pub mod buffer;
pub mod cell;
pub mod clipboard;
pub mod document;
#[cfg(any(target_os = "trueos", test))]
mod editor_background;
pub mod framebuffer;
pub mod fuzzy;
pub mod hash;
pub mod helpers;
pub mod icu;
pub mod input;
pub mod json;
pub mod lsh;
pub mod oklab;
pub mod path;
pub mod simd;
pub mod sys;
pub mod tui;
pub mod unicode;
pub mod vt;
