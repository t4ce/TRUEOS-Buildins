//! Numeric-domain translations of core-js/internals/to-integer-or-infinity.js
//! and to-length.js. The host performs the preceding JS ToNumber conversion;
//! Symbols, BigInts, accessors and object coercion are not modeled here.

pub const MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

/// The numeric body of core-js's ToIntegerOrInfinity helper. Input NaN and both
/// input zeros become positive zero. Truncation of a negative fraction retains
/// negative zero, matching the pinned source's Math.trunc behavior.
pub fn to_integer_or_infinity(number: f64) -> f64 {
    if number.is_nan() || number == 0.0 {
        return 0.0;
    }
    // Clear fractional IEEE-754 bits without requiring std/libm. Infinity and
    // already-integral magnitudes have no fractional bits to clear.
    let bits = number.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023;
    if exponent < 0 {
        return f64::from_bits(bits & (1_u64 << 63));
    }
    if exponent >= 52 {
        return number;
    }
    let fraction = (1_u64 << (52 - exponent as u32)) - 1;
    f64::from_bits(bits & !fraction)
}

/// The numeric body of core-js's ToLength helper: clamp a positive truncated
/// number to 2^53-1; return zero for nonpositive numbers and NaN.
pub fn to_length(number: f64) -> u64 {
    let length = to_integer_or_infinity(number);
    if length <= 0.0 {
        0
    } else if length >= MAX_SAFE_INTEGER as f64 {
        MAX_SAFE_INTEGER
    } else {
        length as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_zero_sign_and_fraction_behavior() {
        for n in [0.0, -0.0, f64::NAN] {
            assert_eq!(to_integer_or_infinity(n).to_bits(), 0);
        }
        assert_eq!(to_integer_or_infinity(-0.5).to_bits(), (-0.0_f64).to_bits());
        assert_eq!(to_integer_or_infinity(3.9), 3.0);
        assert_eq!(to_integer_or_infinity(-3.9), -3.0);
        assert_eq!(to_integer_or_infinity(f64::INFINITY), f64::INFINITY);
        assert_eq!(to_integer_or_infinity(f64::NEG_INFINITY), f64::NEG_INFINITY);
    }
    #[test]
    fn lengths_clamp_at_the_safe_integer_boundary() {
        for n in [f64::NAN, -1.0, f64::NEG_INFINITY, -0.5] {
            assert_eq!(to_length(n), 0);
        }
        assert_eq!(to_length(9.9), 9);
        assert_eq!(
            to_length(MAX_SAFE_INTEGER as f64 - 1.0),
            MAX_SAFE_INTEGER - 1
        );
        assert_eq!(to_length(MAX_SAFE_INTEGER as f64), MAX_SAFE_INTEGER);
        assert_eq!(to_length(f64::MAX), MAX_SAFE_INTEGER);
        assert_eq!(to_length(f64::INFINITY), MAX_SAFE_INTEGER);
    }
}
