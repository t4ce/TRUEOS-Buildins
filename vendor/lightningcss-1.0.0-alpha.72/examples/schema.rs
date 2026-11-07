fn main() {
  #[cfg(feature = "jsonschema")]
  {
    let schema = schemars::schema_for!(lightningcss::stylesheet::StyleSheet);
    let output = serde_json::to_string_pretty(&schema).unwrap();
    let _ = std::fs::write("node/ast.json", output);
  }
}

#[allow(unused_imports)]
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec, borrow::ToOwned};
#[allow(unused_imports)]
use num_traits::Float as _;
