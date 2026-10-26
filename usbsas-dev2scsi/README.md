# usbsas-dev2scsi

usbsas process reading the source USB device. It's the only process with access
to it.

It talks to the device with SCSI commands over USB bulk transfers
(`usbsas-mass-storage`, with libusb on `/dev/bus/usb/BBB/DDD`), without the
kernel `usb_storage` / `uas` drivers, and:

- reads sectors,
- lists partitions (MBR or GPT, with `bootsector`) and detects their file system
  (`FAT`, `exFAT`, `NTFS`, `ext4`, `ISO9660`) and label. A device without
  partition table is reported as a single partition.

It doesn't parse file systems, that's done by its parent, `usbsas-scsi2files`.

## Usage

`usbsas-dev2scsi` takes no argument. It is spawned locked by scsi2files and
waits for its unlock value: the bus number (low 32 bits) and device number (high
32 bits) of the device to open, or `0` to exit without opening anything. Once
the device is opened, it sends `ResponseOpenDevice` (block and device sizes)
without being asked.

## Protocol

`scsi.proto3`, requester: `usbsas-scsi2files`.

| Request       | Response                                       |
|---------------|------------------------------------------------|
| `Partitions`  | partitions of the device (once)                |
| `ReadSectors` | raw sectors (`offset` and `count` in sectors)  |
| `End`         | exit                                           |

## Sandbox

seccomp, with access only to the device opened at unlock.

## Features

- `mock`: read the file given by `USBSAS_MOCK_IN_DEV` instead of a real device
  (see `usbsas-mock`), used by integration tests.
