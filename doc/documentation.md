# usbsas documentation

## Table of contents
* [Build](#build)
* [Usage](#usage)
* [Configuration](#configuration)
* [Tests](#tests)

## Build

### Dependencies

Most dependencies are managed by `cargo` but before building usbsas, the
following packages must also be installed (the names may change depending on the
Linux distribution, see bellow for Debian): `rust`, `cargo`, `pkgconf`, `clang`,
`cmake`, `protobuf`, `seccomp`, `libusb`, `krb5 `, `fuse3 `, `clamav `, `libx11
`, `libxtst `.

A recent version of `rustc` and `cargo` (edition 2021) is needed: instead of a
packaged version, a [rustup](https://rustup.rs/) installation may be necessary.

Already included in the project:
- [FatFs](http://elm-chan.org/fsw/ff/00index_e.html) for reading and writing
  FAT/exFAT file systems (patched source code is located in `ff/src/ff`).
- [ntfs3g](https://github.com/tuxera/ntfs-3g): for writing NTFS file systems.
  (The rust crate used for reading may support writing at some point and
  hopefully ntfs3g won't be needed in the future, patched source code is located
  in `ntfs3g/src/ntfs-3g`).

## Build manually
Install the dependencies and run:

```shell
$ cargo build --release
```

### Build environment variables

`USBSAS_BIN_PATH`: location of the executables (e.g. `/usr/bin/`, default is the
build target directory)

`USBSAS_WEBFILES_DIR`: location of HTML files for the web server (default is
`client/web`).

`USBSAS_CONFIG`: path of the configuration file (default is
`/etc/usbsas/config.toml`)

## Build the debian packages

[cargo-deb](https://github.com/kornelski/cargo-deb#readme) needs to be installed
as well. Packages are provided for the server and the analyzer server.

```shell
$ sudo apt install cargo pkgconf clang cmake git libfuse3-dev libssl-dev libkrb5-dev libclamav-dev libx11-dev libxtst-dev libseccomp-dev
$ cargo install cargo-deb
$ export USBSAS_CONFIG="/etc/usbsas/config.toml"
$ export USBSAS_WEBFILES_DIR="/usr/share/usbsas/web"
$ export USBSAS_BIN_PATH="/usr/libexec"
$ cargo build --release
$ cargo-deb --manifest-path=usbsas-analyzer-server/Cargo.toml
$ cargo-deb --manifest-path=usbsas-server/Cargo.toml
```

/!\ The `usbsas-server` package will create a new user `usbsas` and add a `udev`
rule that will give it ownership of mass storage USB devices (see [USB
permissions](#usb-permissions) bellow).

If you use the analyzer-server, also install `clamav-freshclam` and run `$
freshclam` to download the database.

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

### Web client / server

#### After installing the debian package

Start the servers (analyzer-server can take a moment to load its database, make
sure it exists and is up to date with `freshcalm`):

```shell
$ systemctl start usbsas-analyzer-server
$ systemctl start usbsas-server
```

Start the browser or `nwjs` (for the kiosk mode):
```shell
$ $BROWSER http://localhost:8080
```
or:

```shell
$ nw /usr/share/usbsas/nwjs
```


#### Manually from the source directory
```shell
$ ./target/release/usbsas-server
```
```shell
$ ./target/release/usbsas-analyzer-server
```

```shell
$ $BROWSER http://localhost:8080
```
or:

```shell
$ nw client/nwjs
```

### Other applications

#### Fuse
Build the usbsas-tools crate:
```shell
$ cargo build --release -p usbsas-tools
```
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


## Configuration

See the described `config.example.toml`.


## Tests
### Integration test

Integration tests are written for the `usbsas-server` crate, they test the WEB
API, USB to USB transfer, USB to NET transfer, device wipe etc.

A `mock` feature is available to test the usbsas without real usb devices.

Run the integration tests:
```shell
$ USBSAS_CONFIG=$(pwd)/usbsas-server/test_data/config_test.toml cargo build --features mock
$ USBSAS_CONFIG=$(pwd)/usbsas-server/test_data/config_test.toml cargo test -p usbsas-server
```

### Try usbsas without USB devices

The `mock` feature used for the integration tests also allows using usbsas with
_fake_ (file-backed) USB devices. After building with this feature:

```shell
$ export USBSAS_MOCK_IN_DEV=/tmp/mock_input_dev.img
$ export USBSAS_MOCK_OUT_DEV=/tmp/mock_output_dev.img
$ ./target/debug/usbsas-server
```
