# usbsas build and usage

General instructions to build and use `usbsas`. See also [kiosk](kiosk.md) for
Debian specific instructions.

* [Build](#build)
* [Tests](#tests)
* [Configuration](#configuration)
* [Usage](#usage)

## Build

### Dependencies

Most dependencies are managed by `cargo` but before building usbsas, the
following packages must also be installed (the names may change depending on the
Linux distribution): `rust`, `cargo`, `pkgconf`, `clang`, `cmake`, `protobuf`,
`libseccomp`, `libusb`, `libkrb5 `.

Optional dependencies to build the analyzer-server, the tools and the HID
manager: `libclamav`, `libdbus`, `libxtst`, `libx11`, `libfuse`

A recent version of `rustc` and `cargo` (edition 2021) is needed: instead of a
packaged version, a [rustup](https://rustup.rs/) installation may be necessary.

Already included in the project:
- [FatFs](http://elm-chan.org/fsw/ff/00index_e.html) for reading and writing
  FAT/exFAT file systems (patched source code is located in `ff/src/ff`).
- [ntfs3g](https://github.com/tuxera/ntfs-3g): for writing NTFS file systems.
  (The rust crate used for reading may support writing at some point and
  hopefully ntfs3g won't be needed in the future, patched source code is located
  in `ntfs3g/src/ntfs-3g`).

### Build environment variables

`USBSAS_BIN_PATH`: location of the executables (e.g. `/usr/bin/`, default is the
build target directory)

`USBSAS_WEBFILES_DIR`: location of HTML files for the web server (default is
`client/web`).

`USBSAS_CONFIG`: path of the configuration file (default is
`/etc/usbsas/config.toml`)

### Build
```shell
$ cargo build --release
```
Only `usbsas-server` is built by default, to build the analyzer-server:

```shell
$ cargo build --release -p usbsas-analyzer-server
```
To build the tools (like `usbsas-fuse-mount`):

```shell
$ cargo build --release -p usbsas-tools
```
To build the userland HID manager:
```shell
$ cargo build --release --manifest-path=usbsas-hid/hid-user/Cargo.toml
$ cargo build --release --manifest-path=usbsas-hid/hid-dealer/Cargo.toml
```

## Tests
#### Integration test

Integration tests are written for the `usbsas-server` crate, they test the WEB
API, USB to USB transfer, USB to NET transfer, device wipe etc.

A `mock` feature is available to test the usbsas without real USB devices.

Run the integration tests:
```shell
$ cargo build --all --features mock
$ cargo test -p usbsas-server
```

## Usage

### Requirements

#### Kernel modules

One of the main feature of usbsas is to work in user space, thus the Linux
kernel must not have `usb_storage` and `uas` modules. Either compile a kernel
with `CONFIG_USB_STORAGE` and `CONFIG_USB_UAS` unset or at least prevent this
modules to load because if present, they will be loaded automatically when a USB
device is plugged.

```shell
$ cat << EOF > /etc/modprobe.d/usbsas.conf
install usb_storage /bin/false
blacklist usb_storage
install uas /bin/false
blacklist uas
EOF
$ rmmod usb_storage
$ rmmod uas
$ depmod
```

#### USB permissions

`usbsas` needs R/W permissions on USB devices, multiple options:
- create a specific user, give it ownership of USB devices with a `udev` rule
  and run usbsas with this user.
- OR give ownership of the device to your user: `$ chown user /dev/bus/usb/XXX/YYY`
- OR run it as root (not recommended)


udev rule `/etc/udev/rules.d/99-usbsas.rules`:
```
ACTION=="add", SUBSYSTEM=="usb", ENV{ID_USB_INTERFACES}=="*:080650:*", MODE="0660", OWNER="usbsas"
```

This rule will give ownership of the device to user `usbsas` if the device has
an interface with the class mass storage (0x80), SCSI command set (0x06) and
Bulk transport mode (0x50).

### Configuration
See the described `config.example.toml`.

### Web client / server

After building, start usbsas-server, usbsas-analyzer-server and a web client:

```shell
$ ./target/release/usbsas-server
```
```shell
$ ./target/release/usbsas-analyzer-server
```

```shell
$ $BROWSER http://localhost:8080
```
or with nwjs:

```shell
$ nw client/nwjs
```

The antivirus analysis with the analyzer server is optional. To disable it,
comment or remove the `[analyzer]` section of the `config.toml` file. The
provided analyzer-server based on clamAV is mainly given as example, an
analyzer-server with multiple antiviruses should be preferred.

### Other applications

#### Fuse
After building `usbsas-tools`:
```shell
$ ./target/release/usbsas-fuse-mount --help
usbsas-fuse-mount 1.0
Mount a (fuse) filesystem with usbsas

USAGE:
    usbsas-fuse-mount [OPTIONS] <busnum> <devnum> <mountpoint>

ARGS:
    <busnum>        Bus number of the device to mount
    <devnum>        Dev number of the device to mount
    <mountpoint>    Path to mount the device

OPTIONS:
    -h, --help                  Print help information
    -n, --part-num <PARTNUM>    Partition number to mount [default: 1]
    -V, --version               Print version information
```

#### Python

```shell
$ cd client/python
```

`comm.py` provides a basic class to talk to usbsas with protobuf. A script that
copies everything from a USB device to another (after confirmation) has been
written as an example.

Protobuf python code is generated with `make`, `protobuf` python module and
`protoc` binary should be installed (respectively `python3-protobuf` and
`protobuf-compiler` on Debian).

```shell
$ make
$ python usbsas_transfer_example.py
```

#### Try usbsas without USB devices

The `mock` feature used for the integration tests also allows using usbsas with
_fake_ (file-backed) USB devices. After building with this feature:

```shell
$ export USBSAS_MOCK_IN_DEV=/tmp/mock_input_dev.img
$ export USBSAS_MOCK_OUT_DEV=/tmp/mock_output_dev.img
$ ./target/debug/usbsas-server
```
