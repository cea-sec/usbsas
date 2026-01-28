use std::env;
use std::path::PathBuf;

// Generate bindings, build ff and link statically
fn main() {
    cc::Build::new()
        .compiler("clang")
        .files(&[
            "src/ff/ff.c",
            "src/ff/ffsystem.c",
            "src/ff/ffunicode.c",
            "src/ff/diskio.c",
        ])
        .define("HAVE_CONFIG_H", "")
        .flag("-Wno-unused-parameter")
        .flag("-O2")
        .flag("-D_FORTIFY_SOURCE=2")
        .flag("-fPIC")
        .flag("-fstack-protector-all")
        .flag("-Wformat")
        .flag("-Wformat-security")
        .flag("-Werror=format-security")
        .compile("ff");
    let bindings = bindgen::Builder::default()
        .derive_copy(false)
        .header("src/wrapper.h")
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");

    println!("cargo:rustc-link-lib=static=ff");
    println!(
        "cargo:rustc-link-search=native={}",
        env::var("OUT_DIR").unwrap()
    );
}
