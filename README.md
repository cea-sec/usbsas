<div align="center">
  <p><img src="./doc/res/usbsas-logo.svg"/></p>
  <p>
    <a href="https://github.com/cea-sec/usbsas/actions/workflows/build_check_test.yml?branch=main">
      <img src="https://github.com/cea-sec/usbsas/actions/workflows/build_check_test.yml/badge.svg?branch=main" alt="Build and Test">
    </a>
    <a href="https://www.gnu.org/licenses/gpl-3.0">
      <img src="https://img.shields.io/badge/License-GPLv3-blue.svg">
    </a>
  </p>
</div>

usbsas is a free and open source (GPLv3) tool and framework for securely reading
untrusted USB mass storage devices.


## Overview

Following the concept of defense in depth and the principle of least privilege,
usbsas's goal is to reduce the attack surface of the USB stack. To achieve this,
most of the USB related tasks (parsing USB packets, SCSI commands, file systems
etc.) usually executed in (privileged) kernel space has been moved to user space
and separated in different processes (microkernel style), each being executed in
its own restricted [secure computing
mode](https://en.wikipedia.org/wiki/Seccomp). It works on GNU/Linux and is
written in Rust.

## Use cases

- kiosk / [sheep dip](https://en.wikipedia.org/wiki/Sheep_dip_(computing))
  station to securely transfer files from an untrusted USB device to a trusted
  one
- forensic analysis of untrusted USB devices

## Key features

- read files from an untrusted USB device (without using kernel modules like
  `uas`, `usb_storage` and the file system ones)
- analyze files with a remote antivirus platform
- copy files on a new file system to a trusted USB device
- upload files to a remote server
- make an image of a USB device
- wipe a USB device
- mount (read-only) a USB device

## Documentation

- [Architecture](doc/architecture.md)
- [Features & usage](doc/usage.md)
- [Build](doc/build.md)
- [Configuration](doc/configuration.md)
- [Kiosk deployment](doc/kiosk.md)

## Contributing

Any contribution is welcome, be it code, bug report, packaging, documentation or
translation.

## License

Dependencies included in this project:

- `ntfs3g` is  GPLv2 (see ntfs3g/src/ntfs-3g/COPYING).
- `FatFs` has a custom BSD-style license (see ff/src/ff/LICENSE.txt)

usbsas is free software: you can redistribute it and/or modify it under the
terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version.

usbsas is distributed in the hope that it will be useful, but WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License [along with
usbsas](LICENSE). If not, see [the gnu.org web
site](http://www.gnu.org/licenses/).
