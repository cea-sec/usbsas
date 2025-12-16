# usbsas build

General instructions to build `usbsas`. See also the [kiosk](kiosk.md)
documentation for Debian specific instructions.

* [Dependencies](#dependencies)
* [Build](#build)
* [Tests](#tests)

## Dependencies

Most dependencies are managed by `cargo` but before building usbsas, the
following packages must also be installed (the names may change depending on the
Linux distribution): `rust`, `cargo`, `pkgconf`, `clang`, `cmake`, `protobuf`,
`libseccomp`, `libusb`, `libudev`, `libkrb5 `.

Optional dependencies to build the analyzer-server, the tools and the HID
userland implementation: `libclamav`, `libdbus`, `libxtst`, `libx11`, `libfuse3`

A recent version of `rustc` and `cargo` (edition 2021) is needed: instead of a
packaged version, a [rustup](https://rustup.rs/) installation may be necessary.

To install rust with `rustup`:

```shell
$ curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs
```

Already included in the project:

* [FatFs](http://elm-chan.org/fsw/ff/00index_e.html): for reading and writing
  FAT/exFAT file systems (patched source code is located in `ff/src/ff`).
* [ntfs3g](https://github.com/tuxera/ntfs-3g): for writing NTFS file systems.
  (The rust crate used for reading may support writing at some point and
  hopefully ntfs3g won't be needed in the future, patched source code is located
  in `ntfs3g/src/ntfs-3g`).


## Build

`USBSAS_BIN_PATH`: location of the executables (e.g. `/usr/libexec/`, default is the
build target directory)

`USBSAS_CONFIG`: path of the configuration file (default is
`/etc/usbsas/config.toml`)


Install dependencies listed above (may vary from one distribution to another)
and run:

```shell
$ cargo build --release --all
```

Programs will be located in `target/release`.

## Tests
### Integration tests

Integration tests are written for the `usbsas-usbsas` crate, they test various
transfers: USB to USB transfer, USB to NET transfer, device wipe etc.

A `mock` feature is available to test the usbsas without real USB devices.

Run the integration tests:
```shell
$ cargo build --all --features mock,integration-tests
$ cargo test -p usbsas-usbsas --features integration-tests
```
