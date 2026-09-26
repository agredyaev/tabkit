fn main() {
    println!("cargo:rerun-if-changed=native/hyper_bridge.cpp");
    println!("cargo:rerun-if-env-changed=HYPER_SDK_DIR");
    println!("cargo:rerun-if-env-changed=HYPER_LIB_DIR");
    #[cfg(feature="hyper")]
    build_hyper();
}
#[cfg(feature="hyper")]
fn build_hyper() {
    use std::path::PathBuf;
    let sdk=PathBuf::from(std::env::var_os("HYPER_SDK_DIR").expect("--features hyper requires HYPER_SDK_DIR pointing to the official C++ SDK"));
    let lib=std::env::var_os("HYPER_LIB_DIR").map(PathBuf::from).unwrap_or_else(||sdk.join("lib"));
    assert!(sdk.join("include/hyperapi/hyperapi.hpp").is_file(),"Official Hyper C++ headers were not found");
    cc::Build::new().cpp(true).file("native/hyper_bridge.cpp").include(sdk.join("include"))
    .flag_if_supported("-std=c++17").flag_if_supported("/std:c++17").compile("tabkit_hyper_bridge");
    println!("cargo:rustc-link-search=native={}",lib.display());
    println!("cargo:rustc-link-lib=dylib=tableauhyperapi");
}
