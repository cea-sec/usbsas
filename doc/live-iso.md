# usbsas live-iso

The usbsas live-iso is a live Debian distribution running [usbsas
kiosk](./kiosk.md). It can boot from CD-ROM or USB drive and runs from RAM so
the bootable medium can be safely removed after boot.

The ISO image can be downloaded from the [release
page](https://github.com/cea-sec/usbsas/releases/latest) or built with the
following instructions.

Since it runs from RAM, a machine with at least 4GB is recommended. Half of it
will be dedicated to /tmp (tmpfs) and 3 times the size of a transfer is needed
in /tmp for each transfer.

This live-iso should only be used for testing usbsas, unless it is rebuilt
regularly to have an up-to-date clamAV database.

# Build the image

The ISO image is built with [DebianLive](https://wiki.debian.org/DebianLive)
tools.

```shell
$ cd client/live-iso
$ sudo apt install live-build
$ lb config
$ sudo lb build
```

The resulting image will be `usbsas-0.1.0-amd64.hybrid.iso`.

# How to use

`dd` the image to a USB drive or burn it on a cdrom. Once booted, the bootable
medium can be removed. Since the userland HID manager is used, only basic mouses
and touch screen are supported.
