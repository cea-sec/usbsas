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

    let wp_manifest_path = format!("{}/../Cargo.toml", env::var("CARGO_MANIFEST_DIR").unwrap());
    let usbsas_version =
        toml::from_str::<toml::Table>(&std::fs::read_to_string(&wp_manifest_path).unwrap())
            .unwrap()["workspace"]["metadata"]["version"]
            .as_str()
            .unwrap()
            .to_string();
    println!("cargo:rustc-env=USBSAS_VERSION={usbsas_version}");
    println!("cargo:rerun-if-env-changed=USBSAS_VERSION");
    println!("cargo:rerun-if-env-changed={wp_manifest_path}");
}
