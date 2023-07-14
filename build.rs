use std::path::PathBuf;

fn main() {
    let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("bindings");

    if !out_path.exists() {
        std::fs::create_dir(&out_path).unwrap();
    }

    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if os == "macos" || os == "ios" || os == "watchos" || os == "tvos" {
        let bindings = bindgen::Builder::default()
            .header_contents("wrapper.h", "#include <sys/sysctl.h>")
            .parse_callbacks(Box::new(bindgen::CargoCallbacks))
            .ctypes_prefix("::libc")
            .generate()
            .expect("Unable to generate bindings");

        bindings
            .write_to_file(out_path.join("sysctl.rs"))
            .expect("Couldn't write bindings!");
    }
}
