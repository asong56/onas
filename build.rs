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

    link_ogg();
}

/// `flac-bound` (built with the `libflac-nobuild` feature) links the
/// system's static `libFLAC.a` via pkg-config, but only asks pkg-config
/// for `flac` itself — it never probes or links `ogg`, even though
/// libFLAC's Ogg-container support (`FLAC__ogg_encoder_aspect_*` /
/// `FLAC__ogg_decoder_aspect_*`) is implemented directly against
/// `libogg`'s `ogg_stream_*` / `ogg_sync_*` / `ogg_page_*` symbols.
/// A dynamic link tolerates the missing dependency (the dynamic linker
/// resolves it transitively at load time via `libFLAC.so`'s own
/// `DT_NEEDED` entry), but our fully static link
/// (`-Wl,-Bstatic ... -static-pie`) does not — every symbol used by a
/// linked static archive must be satisfied by another archive named
/// explicitly on the link line. Probe and link `ogg` here so cargo adds
/// `-logg` (after `libFLAC.a`, where it needs to be to satisfy FLAC's
/// references into it).
fn link_ogg() {
    match pkg_config::Config::new().probe("ogg") {
        Ok(_) => {
            // cargo_metadata defaults to true: this alone emits both the
            // `-L` search path and `-logg` link directive.
        }
        Err(e) => {
            // Fall back to a bare `-logg` so the build still has a chance
            // on systems whose libogg ships without a .pc file; if it's
            // not on the default linker search path this will fail with
            // a clearer "cannot find -logg" error instead of silently
            // reproducing the original undefined-symbol failure.
            println!("cargo:warning=pkg-config could not find ogg ({e}); falling back to -logg");
            println!("cargo:rustc-link-lib=ogg");
        }
    }
}
