fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "windows" {
        link_windows();
    } else {
        link_unix();
    }
}

fn link_windows() {
    let vcpkg_root = std::env::var("VCPKG_ROOT")
        .expect("build.rs: VCPKG_ROOT must be set on Windows");
    let base = std::path::PathBuf::from(&vcpkg_root)
        .join("installed")
        .join("x64-windows-static");

    println!("cargo:rustc-link-search=native={}", base.join("lib").display());

    println!("cargo:rustc-link-lib=static=ogg");

    println!("cargo:rustc-link-lib=static=FLAC");
}

fn link_unix() {
    link_via_pkg_config("ogg", "ogg");
    link_via_pkg_config("flac", "FLAC");
}

fn link_via_pkg_config(pc_name: &str, fallback_lib_name: &str) {
    match pkg_config::Config::new().probe(pc_name) {
        Ok(_) => {}
        Err(e) => {
            println!(
                "cargo:warning=pkg-config could not find {pc_name} ({e}); \
                 falling back to bare -l{fallback_lib_name}"
            );
            println!("cargo:rustc-link-lib={fallback_lib_name}");
        }
    }
}
