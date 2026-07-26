fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    match target_os.as_str() {
        "windows" => link_windows(),
        "linux"   => link_pkg_config(),
        "macos"   => link_pkg_config(),
        other     => panic!("x265-sys: unsupported target OS: {}", other),
    }
}

/// Windows: find x265 installed by vcpkg.
/// The CI workflow sets VCPKG_ROOT and installs x265:x64-windows-static.
/// We emit link directives manually so cargo links the static lib.
fn link_windows() {
    // vcpkg static triplet layout:
    //   %VCPKG_ROOT%\installed\x64-windows-static\lib\x265-static.lib
    //   %VCPKG_ROOT%\installed\x64-windows-static\include\x265.h
    let vcpkg_root = std::env::var("VCPKG_ROOT")
        .expect("x265-sys: VCPKG_ROOT must be set on Windows");

    let lib_dir = std::path::PathBuf::from(&vcpkg_root)
        .join("installed")
        .join("x64-windows-static")
        .join("lib");

    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    // x265 static lib name on Windows (vcpkg names it x265-static)
    println!("cargo:rustc-link-lib=static=x265-static");

    // x265 depends on these system libs on Windows
    println!("cargo:rustc-link-lib=user32");
    println!("cargo:rustc-link-lib=gdi32");
    println!("cargo:rustc-link-lib=advapi32");
    println!("cargo:rustc-link-lib=ole32");
}

/// Linux / macOS: use pkg-config to find x265.
fn link_pkg_config() {
    pkg_config::Config::new()
        .atleast_version("3.0")
        .probe("x265")
        .expect(
            "x265-sys: libx265 not found via pkg-config.\n\
             Linux: sudo apt install libx265-dev\n\
             macOS: brew install x265"
        );
}
