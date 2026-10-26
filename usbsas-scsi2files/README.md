# usbsas-scsi2files

usbsas process reading files from the source USB device.

It spawns `usbsas-dev2scsi` (the only process accessing the device) and parses
the file system of the selected partition from the sectors it reads, with the
readers of `usbsas-fsrw`: `FAT`, `exFAT`, `NTFS`, `ext4` and `ISO9660`.

This way the code parsing untrusted file systems has no access to the device,
and the code accessing the device doesn't parse file systems.

## Usage

`usbsas-scsi2files` takes no argument, it's spawned by `usbsas-usbsas`.

## Protocol

`files.proto3`, requester: `usbsas-usbsas`.

1. `OpenDevice { busnum, devnum }`: unlock dev2scsi with this device.
2. `Partitions`: list partitions (`ReadSectors` can also be used here to read
   raw sectors, for disk images).
3. `OpenPartition { index }`: mount the file system of a partition.
4. `GetAttr`, `ReadDir`, `ReadFile` until `End`.

`End` is forwarded to dev2scsi before exiting.

## Sandbox

seccomp, applied right after spawning dev2scsi: the process can only talk to
its parent and to dev2scsi.
