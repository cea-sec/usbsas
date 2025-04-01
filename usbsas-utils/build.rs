use std::env;

fn main() {
    // Get the absolute path of the target directory
    let out_dir = env::var("OUT_DIR")
        .unwrap()
        .split("target")
        .next()
        .unwrap()
        .to_string()
        + "target/"
        + env::var("PROFILE").unwrap().as_str()
        + "/";

    let bin_path = env::var("USBSAS_BIN_PATH").unwrap_or_else(|_| out_dir.clone());
    println!("cargo:rustc-env=USBSAS_BIN_PATH={bin_path}");
    println!("cargo:rerun-if-env-changed=USBSAS_BIN_PATH");

    println!("cargo:rerun-if-env-changed=USBSAS_CONFIG");
}
