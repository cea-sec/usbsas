fn main() {
    let web_files_dir =
        std::env::var("USBSAS_WEBFILES_DIR").unwrap_or_else(|_| "client/web".to_string());
    println!("cargo:rustc-env=USBSAS_WEBFILES_DIR={}", web_files_dir);
    println!("cargo:rerun-if-changed=USBSAS_WEBFILES_DIR");
}
