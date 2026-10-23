//! Embeds `wallflex.manifest` (per-monitor DPI awareness and the supported
//! Windows versions) into the exe when building with the MSVC linker.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=wallflex.manifest");

    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "msvc" {
        let manifest =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("wallflex.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
