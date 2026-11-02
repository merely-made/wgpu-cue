// SPDX-License-Identifier: MPL-2.0
fn main() {
    println!("cargo:rerun-if-changed=src/mach_bridge.c");
    if std::env::var_os("CARGO_FEATURE_METAL_HANDOFF").is_some()
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos")
    {
        cc::Build::new()
            .file("src/mach_bridge.c")
            .flag("-Wno-deprecated-declarations")
            .compile("cue_mach_bridge");
        println!("cargo:rustc-link-lib=bsm");
    }
}
