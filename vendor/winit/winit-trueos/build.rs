#[allow(
    clippy::disallowed_macros,
    reason = "Cargo build scripts report configuration through println!"
)]
fn main() {
    println!("cargo:rustc-check-cfg=cfg(target_os, values(\"trueos\"))");
}
