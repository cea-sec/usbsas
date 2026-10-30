# usbsas build

* [Requirements](#requirements)
* [Building on Debian](#debian)
* [Build options](#options)
* [Building with Docker](#docker)
* [Debian packages](#packages)
* [Live ISO](#liveiso)
* [Tests](#tests)

## <a name="requirements">Requirements</a>

usbsas is built with `cargo`, which fetches the Rust dependencies. It also
needs a few system libraries and tools:

| Dependency | Used by |
|------------|---------|
| Rust toolchain (`rustc`, `cargo`), recent stable, edition 2024 | everything |
| `pkgconf`, `clang`, `cmake`, `make` | building the vendored C libraries (FatFs, ntfs-3g) and the bindings |
| `protobuf-compiler` (`protoc`) | generating the protocol code (`usbsas-proto`) |
| `libseccomp` | sandboxing |
| `libusb-1.0`, `libudev` | accessing USB devices |
| `libssl`, `libkrb5` | network processes (TLS, Kerberos authentication) |
| `libdbus-1` | graphical client |
| `libfuse3` | `usbsas-fuse-mount` tool |
| `clamav-daemon`, `clamav-freshclam` | analyzer server (and integration tests) |

The vendored C libraries are already in the repository:

* [FatFs](http://elm-chan.org/fsw/ff/00index_e.html), to read and write exFAT
  (patched sources in `usbsas-fsrw/ff/src/ff`),
* [ntfs-3g](https://github.com/tuxera/ntfs-3g), to write NTFS (patched sources in
  `usbsas-fsrw/ntfs3g/src/ntfs-3g`).

## <a name="debian">Building on Debian</a>

These instructions target Debian 13 (trixie), the distribution used to build
the official packages. Other distributions work too, with their own package
names.

Install the dependencies:

```shell
$ sudo apt install -y --no-install-recommends \
      curl git make pkgconf clang cmake protobuf-compiler dpkg-dev \
      libseccomp-dev libusb-1.0-0-dev libudev-dev libssl-dev libkrb5-dev \
      libdbus-1-dev libfuse3-dev
```

usbsas uses recent Rust features: if the `rustc` packaged by the distribution
is too old, install the toolchain with [rustup](https://rustup.rs/) instead:

```shell
$ curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then build everything:

```shell
$ git clone https://github.com/cea-sec/usbsas.git
$ cd usbsas
$ cargo build --release --all
```

The programs are in `target/release`. When running them from there, usbsas
looks for its processes in the same directory (see `USBSAS_BIN_PATH` below).

Without `--all`, only the default workspace members are built: the server, its
processes and the client (not the HID driver, the analyzer server nor the
tools). A single component can be built with `-p`:

```shell
$ cargo build --release
$ cargo build --release -p usbsas-hid
```

To try usbsas from the build directory, see [Running from the build
directory](./configuration.md#devrun) in the configuration documentation.

## <a name="options">Build options</a>

### Environment variables

Two paths are set at build time:

| Variable | Default | Description |
|----------|---------|-------------|
| `USBSAS_BIN_PATH` | the cargo target directory (e.g. `target/release`) | Where processes look for the executables of their children. Packages use `/usr/libexec`. |
| `USBSAS_CONFIG` | `/etc/usbsas/config.toml` | Default path of the configuration file. |

```shell
$ USBSAS_BIN_PATH=/usr/libexec USBSAS_CONFIG=/etc/usbsas/config.toml cargo build --release --all
```

### Features

| Feature | Crate | Default | Description |
|---------|-------|---------|-------------|
| `authkrb` | `usbsas-net` | yes | Kerberos authentication with remote servers |
| `native-tls` | `usbsas-net` | no | Use the system TLS library instead of rustls |
| `landlock-enforce` | `usbsas-sandbox` | no | Fail if landlock rules can't be enforced by the kernel, instead of a best effort. Recommended for a kiosk station whose kernel is known. |
| `log-json` | `usbsas-utils` | no | Write logs as JSON objects (one per line), with the session id |
| `mock` | several | no | Replace real USB devices with files, see [Tests](#tests) |
| `integration-tests` | `usbsas-usbsas` | no | Deterministic reports (fixed date and hostname) for the integration tests |

Features of a dependency crate are enabled with its name, for example:

```shell
$ cargo build --release --all --features usbsas-sandbox/landlock-enforce,usbsas-utils/log-json
```

`mock` and `integration-tests` are for tests only and must never be enabled in
a production build.

## <a name="docker">Building with Docker</a>

`debian/Dockerfile` describes the build environment used by the continuous
integration and for releases (Debian trixie with all the dependencies,
`cargo-deb`, `live-build` and ClamAV). Building in this container gives the
same result as the official packages, without installing anything on the host:

```shell
$ docker build -t usbsas-builder:debian -f debian/Dockerfile debian
$ docker run -t -u $(id -u):$(id -g) -v $(pwd):/SRC \
      --env CARGO_HOME=/SRC/target/.cargo \
      usbsas-builder:debian \
      cargo build --release --all
```

## <a name="packages">Debian packages</a>

Packages are built with [cargo-deb](https://github.com/kornelski/cargo-deb):

```shell
$ cargo install cargo-deb
```

Build all the packages with the Makefile (it builds with
`USBSAS_BIN_PATH=/usr/libexec`):

```shell
$ make -C debian pkgs
```

or in the Docker container:

```shell
$ docker run -t -u $(id -u):$(id -g) -v $(pwd):/SRC \
      --env CARGO_HOME=/SRC/target/.cargo \
      usbsas-builder:debian \
      make -C debian pkgs
```

Packages are written in `target/debian`:

| Package | Crate | Content |
|---------|-------|---------|
| `usbsas-server` | `usbsas-usbsas` | server and its processes, configuration file, systemd service, udev rule, modprobe configuration disabling `usb_storage` and `uas` |
| `usbsas-kiosk` | `usbsas-client` | graphical client and its systemd service (in the `cage` kiosk compositor) |
| `usbsas-hid` | `usbsas-hid` | user space HID driver, its service, udev rule, modprobe configuration disabling the kernel HID drivers |
| `usbsas-analyzer-server` | `usbsas-analyzer-server` | demo analyzer server and its service |
| `usbsas-tools` | `usbsas-tools` | standalone tools |

A single package can also be built with `cargo-deb`, but make sure to set
`USBSAS_BIN_PATH` when building the binaries:

```shell
$ USBSAS_BIN_PATH=/usr/libexec cargo build --release --all
$ cargo-deb --no-build -p usbsas-usbsas
```

What each package installs is described in the [kiosk
documentation](./kiosk.md#packages).

## <a name="liveiso">Live ISO</a>

The usbsas live ISO is a live Debian system with the usbsas packages
installed. It boots from a USB drive or a CD-ROM and runs from memory, so the
boot medium can be removed once booted.

**It is only meant to try usbsas, not for production**: `usbsas-hid` isn't
installed, it uses the demo analyzer server, and the system isn't hardened.

It can be downloaded from the [release
page](https://github.com/cea-sec/usbsas/releases/latest) or
built with the following instructions:

```shell
$ sudo apt install live-build
$ make -C debian all
```

The image is `debian/live-iso/usbsas-X.Y.Z-amd64.hybrid.iso`. `live-build`
needs root privileges (the Makefile uses `sudo`).


Write it to a USB drive (with `dd`) or burn it on a
CD-ROM, then boot from it.

Since it runs from memory, a machine with at least 4 GB of RAM is recommended:
half of it is used for `/tmp`, and each transfer needs about 3 times its size
in `/tmp`.


## <a name="tests">Tests</a>

Format and lint checks run by the continuous integration:

```shell
$ cargo fmt --all --check
$ cargo clippy --all
```

Unit tests:

```shell
$ cargo test --all --exclude usbsas-usbsas
```

### Integration tests

The integration tests (`usbsas-usbsas/tests`) run complete sessions: USB to
USB, USB to network, network to USB and USB to command transfers, wipe,
maximum depth... They use the `mock` feature, which replaces real USB devices
with files, and start a local analyzer server, so `clamd` and an up to date
ClamAV database (`freshclam`) are needed.

```shell
$ cargo build --release --all --features mock,integration-tests
$ cargo test --release -p usbsas-usbsas --features integration-tests
```

### Using files as devices

With the `mock` feature, usbsas lists two fake devices backed by files instead
of the plugged USB devices. This is useful for development, or to read a disk
image dumped from a device:

```shell
$ cargo build --release --all --features mock
$ export USBSAS_MOCK_IN_DEV=/path/to/source_image.img
$ export USBSAS_MOCK_OUT_DEV=/path/to/destination_image.img
$ ./target/release/usbsas-usbsas -s
```
