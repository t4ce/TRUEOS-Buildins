#[derive(Debug, PartialEq)]
pub enum PropertyCategory {
  Logical,
  Physical,
}

impl Default for PropertyCategory {
  fn default() -> PropertyCategory {
    PropertyCategory::Physical
  }
}

#[derive(PartialEq)]
pub enum LogicalGroup {
  BorderColor,
  BorderStyle,
  BorderWidth,
  BorderRadius,
  Margin,
  ScrollMargin,
  Padding,
  ScrollPadding,
  Inset,
  Size,
  MinSize,
  MaxSize,
}

#[allow(unused_imports)]
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec, borrow::ToOwned};
#[allow(unused_imports)]
use num_traits::Float as _;
