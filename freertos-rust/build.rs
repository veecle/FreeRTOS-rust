use std::env;
use std::path::PathBuf;

// See: https://doc.rust-lang.org/cargo/reference/build-scripts.html
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Ensures the freeRTOS library is linked explicitly.
    // Without this, the build-success depends on the link order of the build-artifacts which is impacted by the code (e.g. generics, inline,...).
    // Typical errors without explicit linking of the freeRTOS library: "undefined reference to `freertos_rs_get_portTICK_PERIOD_MS'".
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
