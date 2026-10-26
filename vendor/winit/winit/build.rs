use cfg_aliases::cfg_aliases;

#[allow(
    clippy::disallowed_macros,
    reason = "Only relevant for examples and Winit, our usage of println! is fine here."
)]
fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    cfg_aliases! {
        trueos_platform: { target_os = "trueos" },
        linux_platform: { target_os = "linux" },
        x11_platform: { all(feature = "x11", linux_platform) },
        wayland_platform: { all(feature = "wayland", linux_platform) },
    }

    println!("cargo:rustc-check-cfg=cfg(unreleased_changelogs)");
}
