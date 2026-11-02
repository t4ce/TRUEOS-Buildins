//! Portable jQuery 4 CSS algorithms, translated from
//! `src/css/cssCamelCase.js`, `src/css/isAutoPx.js`, `src/var/rcssNum.js`,
//! `src/var/pnum.js`, and the arithmetic branch of `src/css/adjustCSS.js`.
//!
//! Computed style, vendor probing, hooks, and unit conversion need a DOM/style
//! host. In particular this module never substitutes a guessed px/em/% ratio.

use alloc::string::String;

use crate::data::camel_case;

/// CSS's vendor-prefix variant of core camelCase. A host must bypass name
/// normalization for CSS custom properties, as `jQuery.style` itself does.
pub fn css_camel_case(property: &str) -> String {
    if let Some(rest) = property.strip_prefix("-ms-") {
        let mut property = String::from("ms-");
        property.push_str(rest);
        camel_case(&property)
    } else {
        camel_case(property)
    }
}

/// Exact finite grammar from jQuery 4 `isAutoPx`, for already camel-cased names.
/// The initial lowercase ASCII check is significant (`Width` is not admitted).
pub fn is_auto_px(property: &str) -> bool {
    if !property
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_lowercase)
    {
        return false;
    }
    if matches!(
        property,
        "top"
            | "right"
            | "bottom"
            | "left"
            | "width"
            | "height"
            | "minWidth"
            | "minHeight"
            | "maxWidth"
            | "maxHeight"
    ) {
        return true;
    }
    if let Some(rest) = property.strip_prefix("border") {
        return matches!(
            rest,
            "" | "Width"
                | "Top"
                | "TopWidth"
                | "Right"
                | "RightWidth"
                | "Bottom"
                | "BottomWidth"
                | "Left"
                | "LeftWidth"
        );
    }
    if let Some(rest) = property
        .strip_prefix("margin")
        .or_else(|| property.strip_prefix("padding"))
    {
        return matches!(rest, "" | "Top" | "Right" | "Bottom" | "Left");
    }
    false
}

pub fn default_unit(property: &str) -> &'static str {
    if is_auto_px(property) { "px" } else { "" }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelativeOperator {
    Add,
    Subtract,
}

/// Captures of upstream `rcssNum`, with the number converted as by unary `+`.
/// Units retain their source case. This regex is jQuery's numeric token grammar,
/// not a complete CSS value parser (`calc(...)`, `auto`, etc. do not match).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CssNumber<'a> {
    pub relative: Option<RelativeOperator>,
    pub number: f64,
    pub number_text: &'a str,
    pub unit: &'a str,
}

impl<'a> CssNumber<'a> {
    /// Source `valueParts[3] || (isAutoPx(prop) ? "px" : "")`.
    pub fn effective_unit(self, property: &str) -> &'a str {
        if self.unit.is_empty() {
            default_unit(property)
        } else {
            self.unit
        }
    }

    /// Arithmetic after the host has expressed the initial value in the target
    /// unit. An invalid initial number falls back to zero as in `adjustCSS`.
    pub fn endpoint(self, initial_in_target_unit: f64) -> f64 {
        let initial = if initial_in_target_unit.is_nan() || initial_in_target_unit == 0.0 {
            0.0
        } else {
            initial_in_target_unit
        };
        match self.relative {
            Some(RelativeOperator::Add) => initial + self.number,
            Some(RelativeOperator::Subtract) => initial - self.number,
            None => self.number,
        }
    }
}

/// Matches `^(?:([+-])=|)([+-]?(?:\d*\.|)\d+(?:[eE][+-]?\d+|))([a-z%]*)$` /i.
/// An incomplete exponent can be an ASCII unit (`1e` -> number 1, unit `e`),
/// exactly as the source regex's backtracking permits.
pub fn parse_css_number(input: &str) -> Option<CssNumber<'_>> {
    let bytes = input.as_bytes();
    let (relative, mut offset) = if input.starts_with("+=") {
        (Some(RelativeOperator::Add), 2)
    } else if input.starts_with("-=") {
        (Some(RelativeOperator::Subtract), 2)
    } else {
        (None, 0)
    };
    let number_start = offset;
    if matches!(bytes.get(offset), Some(b'+' | b'-')) {
        offset += 1;
    }
    let integer_start = offset;
    while bytes.get(offset).is_some_and(u8::is_ascii_digit) {
        offset += 1;
    }
    let integer_digits = offset - integer_start;
    if bytes.get(offset) == Some(&b'.') {
        offset += 1;
        let fraction_start = offset;
        while bytes.get(offset).is_some_and(u8::is_ascii_digit) {
            offset += 1;
        }
        if offset == fraction_start {
            return None;
        }
    } else if integer_digits == 0 {
        return None;
    }

    if matches!(bytes.get(offset), Some(b'e' | b'E')) {
        let exponent_start = offset;
        offset += 1;
        if matches!(bytes.get(offset), Some(b'+' | b'-')) {
            offset += 1;
        }
        let digits_start = offset;
        while bytes.get(offset).is_some_and(u8::is_ascii_digit) {
            offset += 1;
        }
        if offset == digits_start {
            offset = exponent_start;
        }
    }
    let number_text = &input[number_start..offset];
    let unit = &input[offset..];
    if !unit
        .bytes()
        .all(|byte| byte.is_ascii_alphabetic() || byte == b'%')
    {
        return None;
    }
    Some(CssNumber {
        relative,
        number: number_text.parse().ok()?,
        number_text,
        unit,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleError {
    /// A computed-style host must first convert the initial value to the
    /// requested unit. This module intentionally has no unit conversion table.
    UnitConversionRequired,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedCssNumber<'a> {
    pub number: f64,
    pub unit: &'a str,
}

/// Absolute tokens can directly supply their value/unit. Relative tokens require
/// the host-reported initial unit to match; no computed-style IO occurs here.
pub fn resolve_same_unit<'a>(
    property: &str,
    initial: f64,
    initial_unit: &str,
    target: CssNumber<'a>,
) -> Result<ResolvedCssNumber<'a>, StyleError> {
    let unit = target.effective_unit(property);
    if target.relative.is_some() && initial_unit != unit {
        return Err(StyleError::UnitConversionRequired);
    }
    Ok(ResolvedCssNumber {
        number: target.endpoint(initial),
        unit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_vendor_prefix_is_distinct_from_core_camel_case() {
        assert_eq!(css_camel_case("-ms-transform"), "msTransform");
        assert_eq!(camel_case("-ms-transform"), "MsTransform");
        assert_eq!(css_camel_case("-webkit-transform"), "WebkitTransform");
        assert_eq!(css_camel_case("padding-left"), "paddingLeft");
        assert_eq!(css_camel_case("--custom"), "-Custom");
    }

    #[test]
    fn auto_px_matches_the_exact_upstream_property_grammar() {
        for property in [
            "top",
            "right",
            "bottom",
            "left",
            "width",
            "height",
            "minWidth",
            "maxWidth",
            "minHeight",
            "maxHeight",
            "border",
            "borderWidth",
            "borderTop",
            "borderTopWidth",
            "borderRight",
            "borderRightWidth",
            "borderBottom",
            "borderBottomWidth",
            "borderLeft",
            "borderLeftWidth",
            "margin",
            "marginTop",
            "marginRight",
            "marginBottom",
            "marginLeft",
            "padding",
            "paddingTop",
            "paddingRight",
            "paddingBottom",
            "paddingLeft",
        ] {
            assert!(is_auto_px(property), "{property}");
        }
        for property in [
            "",
            "Width",
            "MarginTop",
            "borderColor",
            "borderRadius",
            "lineHeight",
            "opacity",
            "fontSize",
            "zIndex",
            "widthX",
            "margin-top",
            "--width",
            "maxwidth",
            "borderwidth",
            "paddingWidth",
            "inset",
        ] {
            assert!(!is_auto_px(property), "{property}");
        }
    }

    #[test]
    fn css_numeric_tokens_follow_regex_and_backtracking() {
        for (input, relative, number, unit) in [
            ("1", None, 1.0, ""),
            ("-.5em", None, -0.5, "em"),
            ("+=9px", Some(RelativeOperator::Add), 9.0, "px"),
            ("-=-9px", Some(RelativeOperator::Subtract), -9.0, "px"),
            ("1.25e-2PX", None, 0.0125, "PX"),
            ("1e", None, 1.0, "e"),
            ("1e2e", None, 100.0, "e"),
            ("001%%", None, 1.0, "%%"),
            ("+0", None, 0.0, ""),
            ("+=+.5%", Some(RelativeOperator::Add), 0.5, "%"),
        ] {
            let parsed = parse_css_number(input).unwrap();
            assert_eq!(
                (parsed.relative, parsed.number, parsed.unit),
                (relative, number, unit),
                "{input}"
            );
        }
        for input in [
            "",
            "1.",
            "1.e2",
            ".",
            "--1",
            "1e+",
            "1e-",
            " 1px",
            "1px ",
            "auto",
            "calc(1px + 2px)",
            "10π",
            "Infinity",
            "NaN",
            "+= px",
        ] {
            assert!(parse_css_number(input).is_none(), "{input}");
        }
    }

    #[test]
    fn arithmetic_uses_only_host_confirmed_units() {
        let target = parse_css_number("+=9").unwrap();
        assert_eq!(
            resolve_same_unit("width", 1.0, "px", target),
            Ok(ResolvedCssNumber {
                number: 10.0,
                unit: "px"
            })
        );
        assert_eq!(
            resolve_same_unit("width", 1.0, "em", target),
            Err(StyleError::UnitConversionRequired)
        );
        let target = parse_css_number("-=-9px").unwrap();
        assert_eq!(target.endpoint(1.0), 10.0);
        assert_eq!(target.endpoint(f64::NAN), 9.0);
        let absolute = parse_css_number("2em").unwrap();
        assert_eq!(
            resolve_same_unit("width", 100.0, "px", absolute),
            Ok(ResolvedCssNumber {
                number: 2.0,
                unit: "em"
            })
        );
        assert_eq!(default_unit("opacity"), "");
    }
}
