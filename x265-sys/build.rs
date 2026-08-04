fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    let include_dir = match target_os.as_str() {
        "windows" => link_windows(),
        "linux"   => link_pkg_config(&target_os),
        "macos"   => link_pkg_config(&target_os),
        other     => panic!("x265-sys: unsupported target OS: {}", other),
    };

    // X265_BUILD is the ABI/soname number libx265 splices into the real,
    // versioned `x265_encoder_open_<BUILD>` symbol name — it isn't a
    // plain linkable symbol, and it changes between libx265
    // releases/builds (so it can't be hardcoded without going stale
    // across the different libx265 builds on Windows/Linux/macOS CI).
    // Recover it at build time from whichever headers are actually being
    // linked against; src/lib.rs uses it to call the stable,
    // version-skew-tolerant x265_api_query() instead of linking the
    // versioned symbol directly.
    cc::Build::new()
        .include(&include_dir)
        .file("src/x265_build_probe.c")
        .compile("onas_x265_build_probe");
}

/// Windows: find x265 installed by vcpkg.
/// The CI workflow sets VCPKG_ROOT and installs x265:x64-windows-static.
/// We emit link directives manually so cargo links the static lib.
/// Returns the include directory for the build-time X265_BUILD probe.
fn link_windows() -> std::path::PathBuf {
    // vcpkg static triplet layout:
    //   %VCPKG_ROOT%\installed\x64-windows-static\lib\x265-static.lib
    //   %VCPKG_ROOT%\installed\x64-windows-static\include\x265.h
    let vcpkg_root = std::env::var("VCPKG_ROOT")
        .expect("x265-sys: VCPKG_ROOT must be set on Windows");
    let base = std::path::PathBuf::from(&vcpkg_root)
        .join("installed")
        .join("x64-windows-static");

    println!("cargo:rustc-link-search=native={}", base.join("lib").display());

    // x265 static lib name on Windows (vcpkg names it x265-static)
    println!("cargo:rustc-link-lib=static=x265-static");

    // x265 depends on these system libs on Windows
    println!("cargo:rustc-link-lib=user32");
    println!("cargo:rustc-link-lib=gdi32");
    println!("cargo:rustc-link-lib=advapi32");
    println!("cargo:rustc-link-lib=ole32");

    base.join("include")
}

/// Linux / macOS: use pkg-config to find x265.
/// Returns an include directory for the build-time X265_BUILD probe.
///
/// The final `onas` binary is linked fully statically on Linux
/// (`-Wl,-Bstatic ... -static-pie`), so cargo needs to know about
/// libx265's *own* link-time dependencies there, not just `-lx265`
/// itself. Ubuntu/Debian's static `libx265.a` is built with NUMA
/// thread-affinity support, so it references `numa_available` /
/// `numa_allocate_nodemask` / etc. from `libnuma` — symbols a plain
/// (non-static) pkg-config probe never surfaces, because that only reads
/// the public `Libs:` line (just `-lx265`), not the `Libs.private:` line
/// where `-lnuma` lives. Asking pkg-config for the *static* link line
/// (`.statik(true)`, i.e. `pkg-config --static`) makes it also emit
/// `Libs.private`, so `-lnuma` (and any other static-only dependency)
/// reaches the final `cc` call.
///
/// NUMA is a Linux-only concept — there is no `libnuma` on macOS, and
/// Homebrew's x265 formula is built without NUMA support entirely — so
/// the `-lnuma` fallback below must not fire there; doing so is exactly
/// what caused `ld: library 'numa' not found` on `aarch64-apple-darwin`.
fn link_pkg_config(target_os: &str) -> std::path::PathBuf {
    let lib = pkg_config::Config::new()
        .atleast_version("3.0")
        .statik(true)
        .probe("x265")
        .expect(
            "x265-sys: libx265 not found via pkg-config.\n\
             Linux: sudo apt install libx265-dev libnuma-dev\n\
             macOS: brew install x265"
        );

    if target_os == "linux" {
        // Belt-and-suspenders: some distro .pc files simply omit
        // `Libs.private` (it's optional metadata, easy to forget when
        // packaging), so the `--static` probe above can still come back
        // without `-lnuma` even though the static archive needs it. Link
        // it explicitly too; this is a harmless no-op on any Linux system
        // where libx265 happens to have been built without NUMA support
        // (the symbols just go unreferenced in that case).
        println!("cargo:rustc-link-lib=numa");
    }

    lib.include_paths.into_iter().next()
        .expect("x265-sys: pkg-config returned no include path for x265")
}
