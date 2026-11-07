//! The renderer uses the same core/alloc libm operations on native and host builds.
pub use libm::{ceil, cos, round, sin, sqrt};
#[inline]
pub fn square(value: f64) -> f64 {
    value * value
}
