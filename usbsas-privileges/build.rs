fn main() {
    // Compile a bit of C to get USBDEVFS consts because bindgen can't do it yet
    // see https://github.com/rust-lang/rust-bindgen/issues/753
    cc::Build::new().file("src/usbdevfs.c").compile("usbdevfs");
}
