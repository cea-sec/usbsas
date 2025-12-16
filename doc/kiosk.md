# usbsas kiosk

Here are instructions on how to deploy usbsas as a kiosk / sheep-dip station,
e.g. a system that boots directly to usbsas's graphical user interface without
the possibility to use or run other applications.

* [Debian packages](#deb)
    * [Build](#build)
    * [usbsas-server](#debserver)
    * [usbsas-kiosk](#debkiosk)
    * [usbsas-analyzer-server](#debanalyzer)
    * [usbsas-hid](#debhid)
* [Installation](#installation)
* [Live ISO](#liveiso)


## <a name="deb">Debian packages</a>

Debian packages (for x86_64) can be downloaded from the [release
page](https://github.com/cea-sec/usbsas/releases/latest) or built with the
following instructions.

### Build

First, cargo-deb need to be installed:

```shell
$ cargo install cargo-deb
```

Dependencies:

```shell
$ sudo apt install -y --no-install-recommends \
      pkgconf \
      clang \
      cmake \
      git \
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

Packages can be built individually with cargo-deb:

```shell
$ cargo-deb -p usbsas-usbsas
$ cargo-deb -p usbsas-client
$ cargo-deb -p usbsas-analyzer-server
$ cargo-deb -p usbsas-hid
```

or with the Makefile:

```shell
$ make -C debian pkgs
```

Packages will be located in `target/debian`.


### <a name="debserver">usbsas-server package</a>

This package contains usbsas processes, configuration file and a `systemd`
service.

Upon installation, a new user `usbsas` will be added and a `udev` rule giving
ownership of plugged USB devices to this user. usbsas server side processes will
run with this user.

A modprobe configuration file preventing `uas` and `usb_storage` kernel modules
from loading will also be installed.

### <a name="debkiosk">usbsas-kiosk (usbsas-client) package</a>

This package contains usbsas graphical interface, a `systemd` service and a
`xinit` script.

The systemd service, when enabled, will automatically start the application at
boot (through `xinit`).

Upon installation, a new user `usbsas-client` will be added, the graphical
interface will run with this user.

This package depends on the `usbsas-server` package.

### <a name="debanalyzer">usbsas-analyzer-server package</a>

This package contains the **demo** analyzer server.

It will install clamav-daemon and clamav-freshclam as dependencies.

### <a name="debhid">usbsas-hid package</a>

This package contains the minimal HID implementation running in user space,
it only supports mouse left click (no keyboard).

It is started by the graphical user interface if it is installed.

`hid` kernel modules are prevented from loading with a modprobe configuration file.

A `udev` rule will give ownership of HID devices to `usbsas-client` when plugged
and start the HID manager.

The installation of `usbsas-hid` is recommended but not mandatory.

## Installation

/!\ Warning: Installing these packages will confine the system to the graphical
user interface and since keyboards will be disabled (if usbsas-hid is
installed), it is a good idea to keep an access (ssh for example) to the
machine.

Based on a fresh Debian installation (everything as default, no desktop
environment), install the packages:

```shell
$ sudo apt install ./usbsas-server_X.Y.Z_amd64.deb \
                   ./usbsas-analyzer-server_X.Y.Z_amd64.deb \
                   ./usbsas-kiosk_X.Y.Z_amd64.deb \
                   ./usbsas-hid_X.Y.Z_amd64.deb
```

Installing the demo analyzer-server will install clamav-freshclam which needs
internet to download its virus database.

clamav-daemon should be disabled since the analyzer-server forks its owns
clamav:

```shell
sudo systemctl disable clamav-daemon.service
```

Enable the services and reboot:

```shell
sudo systemctl enable usbsas-server.service
sudo systemctl enable usbsas-analyzer-server.service
sudo systemctl enable usbsas-client.service
sudo reboot
```

The system should reboot directly to the graphical user interface.


### <a name="liveiso">Live ISO</a>

The usbsas live-iso is a live Debian distribution running with usbsas packages
installed.

It can boot from CD-ROM or USB drive and runs from RAM so
the bootable medium can be safely removed after boot.

The ISO image can be downloaded from the [release
page](https://github.com/cea-sec/usbsas/releases/latest) or built with the
following instructions.

Since it runs from RAM, a machine with at least 4GB is recommended. Half of it
will be dedicated to /tmp (tmpfs) and 3 times the size of a transfer is needed
in /tmp for each transfer.

This live-iso should only be used to try usbsas.

#### Build the image

The ISO image is built with [DebianLive](https://wiki.debian.org/DebianLive)
tools, install the `live-build` pkg and run `make`.

```shell
$ sudo apt install live-build
$ make -C debian all
```

The resulting image will be `usbsas-X.Y.Z-amd64.hybrid.iso`.

#### How to use

`dd` the image to a USB drive or burn it on a cdrom. Once booted, the bootable
medium can be removed.




