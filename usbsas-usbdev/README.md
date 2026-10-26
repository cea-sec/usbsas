# usbsas-usbdev

usbsas process listing plugged USB mass storage devices.

A thread scans the devices already plugged at startup, then follows udev events
to keep the list up to date. Only mass storage devices using the SCSI command
set with bulk transport are listed (interface `08/06/50`), plus optical disk
readers (`08/02/50`), which can only be sources.

If `[usb_port_accesses]` is configured, each device is marked as a possible
source and / or destination depending on the port it's plugged in, and devices
on other ports are ignored.

## Usage

```
usbsas-usbdev [-c CONFIG_PATH]
```

## Protocol

`usbdev.proto3`, requester: `usbsas-usbsas`.

| Request   | Response                          |
|-----------|-----------------------------------|
| `Devices` | currently plugged devices         |
| `End`     | exit                              |

## Sandbox

landlock (read only access to the configuration, `/sys` and `/run/udev`) and
seccomp. The udev thread has its own seccomp filter, and parses the
configuration after entering it.

## Features

- `mock`: replace the real implementation with `usbsas-mock`'s fake device list,
  used by integration tests.
