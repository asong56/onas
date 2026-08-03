//! Recovers `VPX_DECODER_ABI_VERSION` at build time.
//!
//! That constant is computed via preprocessor arithmetic in libvpx's
//! `vpx_decoder.h` and isn't captured in the pre-generated bindings the
//! `libvpx-sys` crate ships (rust-vpx repo, pinned commit 04df690), so
//! `vpx_codec_dec_init_ver()` in src/video.rs has no way to get it from
//! `vpx_sys` directly. Probing it from whichever libvpx headers are
//! actually linked (vcpkg on Windows, system package elsewhere) keeps
//! this correct across libvpx versions instead of hardcoding a number
//! that would go stale.

fn main() {
    let lib = pkg_config::Config::new()
        .cargo_metadata(false) // libvpx-sys already emits the link directives
        .probe("vpx")
        .expect("pkg-config could not find vpx (needed to read VPX_DECODER_ABI_VERSION)");

    let mut build = cc::Build::new();
    for inc in &lib.include_paths {
        build.include(inc);
    }
    build
        .file("src/vpx_abi_probe.c")
        .compile("onas_vpx_abi_probe");
}
