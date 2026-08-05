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

    // `ogg` — see `link_via_pkg_config` below. libFLAC's Ogg-container
    // support (`FLAC__ogg_encoder_aspect_*` / `FLAC__ogg_decoder_aspect_*`)
    // is implemented directly against libogg's `ogg_stream_*` /
    // `ogg_sync_*` / `ogg_page_*` symbols, but `libflac-sys` (see below)
    // never probes or links `ogg` itself.
    link_via_pkg_config("ogg", "ogg");

    // `libde265` — the `libde265` crate's default `system` feature
    // (`libde265-sys`'s `link_system_libde265()`) only ever emits
    // `cargo:rustc-link-lib=dylib=de265`; it never runs pkg-config, so it
    // never emits a `-L` search path either.
    link_via_pkg_config("libde265", "de265");

    // `flac` — `flac-bound`'s `libflac-nobuild` feature enables
    // `libflac-sys` with its `build-flac` feature *off*, which likewise
    // only emits `cargo:rustc-link-lib=FLAC` and nothing else.
    link_via_pkg_config("flac", "FLAC");
}

/// Several dependency `-sys` crates (`libde265-sys`'s `system` feature,
/// `libflac-sys` without `build-flac`) link a system C library by emitting
/// a bare `cargo:rustc-link-lib=<name>` and nothing else — i.e. "link
/// against whatever the system already provides" — without ever calling
/// pkg-config themselves, so they never emit a `-L` search-path directive.
///
/// On Linux this still works because apt installs shared libraries into
/// the multiarch directories (e.g. `/usr/lib/x86_64-linux-gnu`) the system
/// linker already searches by default. On both Homebrew (macOS) and vcpkg
/// (Windows), libraries live under a package-specific prefix
/// (`/opt/homebrew/Cellar/<name>/<ver>/lib`,
/// `%VCPKG_ROOT%\installed\x64-windows-static\lib`) that is *not* on the
/// linker's default search path, so the exact same bare `-l<name>` fails
/// at link time — `ld: library 'de265' not found` on macOS, and the MSVC
/// equivalent on Windows — even though `brew install <name>` / `vcpkg
/// install <name>` completed successfully. This is the same class of bug
/// this file already worked around for `ogg` (see the call sites above);
/// generalized here to also cover `libde265` and `flac`, which hit it too
/// but hadn't surfaced yet because the link stops at the first missing
/// library it hits.
///
/// Probing here and letting `pkg-config` emit the search path closes the
/// gap without patching either dependency. `probe()`'s default
/// `cargo_metadata(true)` emits both `-L` and `-l` straight from the
/// library's own `Libs:` line, so this is also robust to a library whose
/// linkable name doesn't match its pkg-config module name — e.g. module
/// `libde265` (`libde265.pc`), linkable library `de265` (`-lde265`).
/// `pc_name` is the pkg-config module to probe for; `fallback_lib_name` is
/// the bare library name to fall back to (matching what the dependency's
/// own build script already links) if that probe fails.
fn link_via_pkg_config(pc_name: &str, fallback_lib_name: &str) {
    match pkg_config::Config::new().probe(pc_name) {
        Ok(_) => {
            // cargo_metadata defaults to true: this alone emits both the
            // `-L` search path and the `-l<fallback_lib_name>` link
            // directive, read straight from the `.pc` file's `Libs:` line.
        }
        Err(e) => {
            // Fall back to a bare `-l<fallback_lib_name>` so the build
            // still has a chance on systems whose library ships without a
            // `.pc` file; if it's not on the default linker search path
            // this will fail with a clearer "cannot find" error instead of
            // silently reproducing the original undefined-symbol failure.
            println!(
                "cargo:warning=pkg-config could not find {pc_name} ({e}); falling back to bare -l{fallback_lib_name}, which requires the library to already be on the linker's default search path"
            );
            println!("cargo:rustc-link-lib={fallback_lib_name}");
        }
    }
}
