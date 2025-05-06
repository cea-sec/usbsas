# usbsas kiosk

* [Build packages](#build-packages)
* [Installation](#installation)

usbsas is meant to be deployed as a kiosk station. Here is a guide to do it
based on a fresh Debian installation (everything as default, no desktop
environment).

Debian packages (for x86_64) can be downloaded from the [release
page](https://github.com/cea-sec/usbsas/releases/latest) or built with the
following instructions.

## Build packages

Install dependencies:
```shell
$ sudo apt install -y --no-install-recommends \
      pkgconf \
      clang \
      cmake \
      git \
      curl \
      dpkg-dev \
      libssl-dev \
      libkrb5-dev \
      libseccomp-dev \
      libudev-dev \
      libusb-1.0-0-dev \
      protobuf-compiler \
      libdbus-1-dev \
      libxtst-dev \
      libx11-dev
```

Install rust and cargo-deb (to build Debian packages from Cargo.toml
instructions):
```shell
$ curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
$ source $HOME/.cargo/env
$ cargo install cargo-deb
```

Clone and build usbsas:
```shell
$ git clone https://github.com/cea-sec/usbsas
$ cd usbsas
$ export USBSAS_BIN_PATH="/usr/libexec"
$ cargo build --release
$ cargo build --release -p usbsas-client
$ cargo build --release -p usbsas-analyzer-server
$ cargo build --release --manifest-path=usbsas-hid/hid-user/Cargo.toml
$ cargo build --release --manifest-path=usbsas-hid/hid-dealer/Cargo.toml
```

Build packages:
```shell
$ cargo-deb --manifest-path=usbsas-usbsas/Cargo.toml --no-build
$ cargo-deb --manifest-path=usbsas-client/Cargo.toml --no-build
$ cargo-deb --manifest-path=usbsas-analyzer-server/Cargo.toml --no-build
$ cargo-deb --manifest-path=usbsas-hid/hid-dealer/Cargo.toml --no-build
```

The `usbsas-core` package contains usbsas processes. It will add a new user
`usbsas` and a udev rule giving it ownership of plugged USB devices. `uas` and
`usb_storage` kernel modules are prevented from loading with a modprobe
configuration file.

The `usbsas-kiosk` (usbsas-client) package contains the GUI client and a script
meant to be started by xinit at boot. The systemd service, when enabled, will
automatically start the application at boot.

The `usbsas-analyzer-server` package contains the analyzer server. It will
install clamav-daemon and clamav-freshclam as dependencies.

The `usbsas-hid` package contains a minimal HID manager running in user space,
it only supports mouse left click (no keyboard). `hid` kernel modules are
prevented from loading with a modprobde configuration file. A udev rule will
give ownership of HID devices to `usbsas-client` when plugged and start the HID
manager. The installation of `usbsas-hid` is recommended but not mandatory.

## Installation

Built packages are located in `target/debian`
```shell
$ sudo apt install ./usbsas-core_X.Y.Z_amd64.deb \
                   ./usbsas-analyzer-server_X.Y.Z_amd64.deb \
                   ./usbsas-kiosk_X.Y.Z_amd64.deb \
                   ./usbsas-hid_X.Y.Z_amd64.deb
```

Installing the analyzer-server will install clamav-freshclam which needs
internet to download its virus database.

After installation, systemd services must be enabled and a reboot is needed.

/!\ Warning: Once the system has rebooted, the only displayed application will
be the web client and since keyboards will be disabled (if usbsas-hid is
installed), it is a good idea to keep an access (ssh for example) to the
machine.

```shell
sudo systemctl disable clamav-daemon.service
sudo systemctl enable usbsas-server.service
sudo systemctl enable usbsas-analyzer-server.service
sudo systemctl enable usbsas-client.service
sudo reboot
```

usbsas native client:

<p align="center"><img src="./client_screenshot.png" width="100%"/></p>

## Hardening

XXX TODO
