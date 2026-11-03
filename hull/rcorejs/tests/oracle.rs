//! Fixtures are recorded by executing the untouched pinned core-js helpers and
//! forced fallback string methods. No Rust test dependencies or JS heap model.
use rcorejs::{
    abstract_ops::{to_integer_or_infinity, to_length},
    string::{is_well_formed, to_well_formed},
};

#[test]
fn numeric_operations_match_source_including_signed_zero_and_ieee_extremes() {
    let mut count = 0;
    for line in include_str!("fixtures/numbers.tsv")
        .lines()
        .filter(|s| !s.starts_with('#'))
    {
        let fields = line.split('\t').collect::<Vec<_>>();
        let input = f64::from_bits(u64::from_str_radix(fields[0], 16).unwrap());
        assert_eq!(
            to_integer_or_infinity(input).to_bits(),
            u64::from_str_radix(fields[1], 16).unwrap(),
            "input bits {}",
            fields[0]
        );
        assert_eq!(
            to_length(input),
            fields[2].parse::<u64>().unwrap(),
            "input bits {}",
            fields[0]
        );
        count += 1;
    }
    assert!(count >= 1000);
}
fn units(text: &str) -> Vec<u16> {
    if text == "-" {
        Vec::new()
    } else {
        text.split(',')
            .map(|s| u16::from_str_radix(s, 16).unwrap())
            .collect()
    }
}
#[test]
fn utf16_operations_match_original_fallback_bodies() {
    let mut count = 0;
    for line in include_str!("fixtures/utf16.tsv")
        .lines()
        .filter(|s| !s.starts_with('#'))
    {
        let fields = line.split('\t').collect::<Vec<_>>();
        let input = units(fields[0]);
        assert_eq!(
            is_well_formed(&input),
            fields[1].parse::<bool>().unwrap(),
            "input {:?}",
            input
        );
        assert_eq!(
            to_well_formed(&input),
            units(fields[2]),
            "input {:?}",
            input
        );
        count += 1;
    }
    assert!(count >= 1500);
}
