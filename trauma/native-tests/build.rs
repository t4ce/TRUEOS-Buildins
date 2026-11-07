use std::{env, fs, path::PathBuf};
fn main() {
    // Select the real target branches for deterministic host-native fixtures.
    // Only the cfg selector and the module's own hosted tests are adjusted.
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for name in ["body", "checkpoint"] {
        let path = format!("../src/{name}.rs");
        println!("cargo:rerun-if-changed={path}");
        let source = fs::read_to_string(path).unwrap();
        let source = source
            .split("\n#[cfg(test)]\nmod tests")
            .next()
            .unwrap()
            .replace("target_os = \"trueos\"", "feature = \"native-fixture\"")
            .replace("//!", "//");
        fs::write(output.join(format!("{name}.rs")), source).unwrap();
    }
}
