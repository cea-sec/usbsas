# usbsas-hid

Minimal user space HID driver, to protect a kiosk against BadUSB devices.

The kernel HID drivers are disabled (`assets/hid.conf`). Instead:

- `hid-dealer` monitors udev and starts a `hid-user` process for each HID
  device plugged, on an allowed port if `ports_hid` is set in
  `[usb_port_accesses]`. It stops it when the device is unplugged.
- `hid-user` opens the device with libusb, parses its HID report descriptor and
  replays the reports of mice and touch screens on a virtual `uinput` device.
  Only pointer motion and left click are forwarded: **keystrokes are never
  handled**, so a malicious device pretending to be a keyboard can't type
  anything.

Left click is enough to use the usbsas client.

## Sandbox

- `hid-dealer`: landlock, read only access to the configuration, `/sys`, `/proc`,
  `/run/udev` and the executables, read / write access to `/dev/bus/usb` and
  `/dev/uinput`.
- `hid-user`: seccomp once the device and `/dev/uinput` are opened: it can only
  use the device with libusb and write events to uinput.

## Packaging

`cargo deb -p usbsas-hid` builds the `usbsas-hid` package, see [kiosk
deployment](../doc/kiosk.md#debhid). It installs:

- a systemd unit running `hid-dealer` as the `usbsas-hid` user,
- a udev rule giving this user access to HID devices and to `/dev/uinput`,
- a modprobe configuration disabling the kernel HID drivers, and a
  `modules-load.d` file loading `uinput`.
