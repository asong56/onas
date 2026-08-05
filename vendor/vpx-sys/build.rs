extern crate pkg_config;

fn main() {
    pkg_config::Config::new().statik(true).probe("vpx").expect("libvpx not found via pkg-config");
}
