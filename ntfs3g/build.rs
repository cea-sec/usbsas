use std::env;
use std::path::PathBuf;

fn main() {
    cc::Build::new()
        .compiler("clang")
        .files(&[
            "src/ntfs-3g/acls.c",
            "src/ntfs-3g/attrib.c",
            "src/ntfs-3g/attrdef.c",
            "src/ntfs-3g/attrlist.c",
            "src/ntfs-3g/bitmap.c",
            "src/ntfs-3g/boot.c",
            "src/ntfs-3g/bootsect.c",
            "src/ntfs-3g/cache.c",
            "src/ntfs-3g/collate.c",
            "src/ntfs-3g/compat.c",
            "src/ntfs-3g/compress.c",
            "src/ntfs-3g/debug.c",
            "src/ntfs-3g/device.c",
            "src/ntfs-3g/dir.c",
            "src/ntfs-3g/ea.c",
            "src/ntfs-3g/efs.c",
            "src/ntfs-3g/index.c",
            "src/ntfs-3g/inode.c",
            "src/ntfs-3g/ioctl.c",
            "src/ntfs-3g/lcnalloc.c",
            "src/ntfs-3g/logfile.c",
            "src/ntfs-3g/logging.c",
            "src/ntfs-3g/mft.c",
            "src/ntfs-3g/misc.c",
            "src/ntfs-3g/mkntfs.c",
            "src/ntfs-3g/mst.c",
            "src/ntfs-3g/object_id.c",
            "src/ntfs-3g/realpath.c",
            "src/ntfs-3g/reparse.c",
            "src/ntfs-3g/runlist.c",
            "src/ntfs-3g/security.c",
            "src/ntfs-3g/sd.c",
            "src/ntfs-3g/unistr.c",
            "src/ntfs-3g/unix_io.c",
            "src/ntfs-3g/utils.c",
            "src/ntfs-3g/volume.c",
            "src/ntfs-3g/xattrs.c",
        ])
        .define("HAVE_CONFIG_H", "1")
        .flag("-Wno-unused-parameter")
        .flag("-Wno-implicit-function-declaration")
        .flag("-Wno-address-of-packed-member")
        .flag("-Wno-unused-but-set-variable")
        .flag("-Wno-unknown-warning-option")
        .flag("-O2")
        .flag("-D_FORTIFY_SOURCE=2")
        .flag("-fPIC")
        .flag("-fstack-protector-all")
        .flag("-Wformat")
        .flag("-Wformat-security")
        .flag("-Werror=format-security")
        .compile("ntfs3g");
    let bindings = bindgen::Builder::default()
        .header("src/wrapper.h")
        .derive_copy(false)
        .derive_debug(false)
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");

    println!("cargo:rustc-link-lib=static=ntfs3g");
    println!(
        "cargo:rustc-link-search=native={}",
        env::var("OUT_DIR").unwrap()
    );
}
