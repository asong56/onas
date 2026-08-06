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

    println!("cargo:rustc-link-lib=static=libde265");

    println!("cargo:rustc-link-lib=static=FLAC");

    cc::Build::new()
        .include(base.join("include"))
        .file("src/vpx_abi_probe.c")
        .compile("onas_vpx_abi_probe");
}

fn link_unix() {
    let lib = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("vpx")
        .expect("pkg-config could not find vpx");

    let mut build = cc::Build::new();
    for inc in &lib.include_paths {
        build.include(inc);
    }
    build
        .file("src/vpx_abi_probe.c")
        .compile("onas_vpx_abi_probe");

    link_via_pkg_config("ogg", "ogg");
    link_via_pkg_config("libde265", "de265");
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
