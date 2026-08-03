fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    let include_dir = match target_os.as_str() {
        "windows" => link_windows(),
        "linux"   => link_pkg_config(),
        "macos"   => link_pkg_config(),
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
fn link_pkg_config() -> std::path::PathBuf {
    let lib = pkg_config::Config::new()
        .atleast_version("3.0")
        .probe("x265")
        .expect(
            "x265-sys: libx265 not found via pkg-config.\n\
             Linux: sudo apt install libx265-dev\n\
             macOS: brew install x265"
        );
    lib.include_paths.into_iter().next()
        .expect("x265-sys: pkg-config returned no include path for x265")
}
