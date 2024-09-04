use std::env;
use std::path::PathBuf;

// See: https://doc.rust-lang.org/cargo/reference/build-scripts.html
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Ensures the freeRTOS library is linked explicitly.
    println!("cargo::rustc-link-lib=static:-bundle=freertos");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    println!(
        "cargo:SHIM={}",
        PathBuf::from(manifest_dir)
            .join("src/freertos")
            .to_str()
            .unwrap()
    );
}
