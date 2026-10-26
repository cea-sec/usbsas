# usbsas-tools

Standalone command line tools built from usbsas components. They show how the
processes can be reused outside of a full usbsas transfer. See the [usage
documentation](../doc/usage.md#tools) for their options.

| Tool                 | Description                                                   | Processes used |
|----------------------|---------------------------------------------------------------|----------------|
| `usbsas-fuse-mount`  | mount a partition of a USB device, read only, with FUSE        | scsi2files (and dev2scsi) |
| `usbsas-imager`      | copy a whole USB device to a file or stdout (like `dd`)       | dev2scsi       |
| `usbsas-fswriter`    | write a file system image to a USB device (like `dd`)         | fs2dev         |
| `usbsas-net`         | test uploading, analyzing or downloading a bundle with a remote server | uploader, analyzer, downloader |
| `usbsas-devmonitor`  | print plugged / unplugged USB devices with their port path, to configure `[usb_port_accesses]` | - |

Devices are designated by their bus and device numbers (`lsusb`). As in usbsas,
the kernel mass storage drivers aren't used, and the tools apply seccomp
filters themselves (imager, fswriter) once their children are spawned.

## Features

Each tool has its own feature (`imager`, `fswriter`, `fuse-mount`, `net`,
`devmonitor`), all enabled by default.
