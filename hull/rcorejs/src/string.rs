//! UTF-16 bodies from core-js/modules/es.string.{is,to}-well-formed.js.
//! JS strings are code-unit sequences, so &[u16] retains lone surrogates that
//! Rust &str cannot represent. JS receiver checks/ToString remain host work.
use alloc::vec::Vec;

pub fn is_well_formed(units: &[u16]) -> bool {
    let mut i = 0;
    while i < units.len() {
        let unit = units[i];
        if unit & 0xf800 == 0xd800 {
            if unit >= 0xdc00 || i + 1 >= units.len() || units[i + 1] & 0xfc00 != 0xdc00 {
                return false;
            }
            i += 1;
        }
        i += 1;
    }
    true
}

/// Replace each unpaired surrogate with U+FFFD; retain valid pairs and every
/// other code unit. Result length equals input length, without modifying input.
pub fn to_well_formed(units: &[u16]) -> Vec<u16> {
    let mut result = units.to_vec();
    let mut i = 0;
    while i < units.len() {
        let unit = units[i];
        if unit & 0xf800 == 0xd800 {
            if unit >= 0xdc00 || i + 1 >= units.len() || units[i + 1] & 0xfc00 != 0xdc00 {
                result[i] = 0xfffd;
            } else {
                i += 1;
            }
        }
        i += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paired_and_unpaired_code_units_are_distinct() {
        assert!(is_well_formed(&[]));
        assert!(is_well_formed(&[0x61, 0xd83d, 0xdca9, 0x62]));
        assert!(!is_well_formed(&[0xdca9, 0xd83d]));
        assert_eq!(to_well_formed(&[0xdca9, 0xd83d]), [0xfffd, 0xfffd]);
        let input = [0xd800, 0xd800, 0xdc00, 0xdc00];
        assert_eq!(to_well_formed(&input), [0xfffd, 0xd800, 0xdc00, 0xfffd]);
        assert_eq!(input, [0xd800, 0xd800, 0xdc00, 0xdc00]);
    }
    #[test]
    fn repair_preserves_length_and_is_idempotent() {
        for input in [&[0xd800, 0x61, 0xdc00][..], &[0, 0xd7ff, 0xe000, 0xffff], &[]] {
            let output = to_well_formed(input);
            assert_eq!(input.len(), output.len());
            assert!(is_well_formed(&output));
            assert_eq!(to_well_formed(&output), output);
        }
    }
}
