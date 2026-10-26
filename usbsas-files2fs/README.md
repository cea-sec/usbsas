# usbsas-files2fs

usbsas process creating a file system image to be written on the destination
USB device.

It writes into a file on disk (`usbsas_<id>.img`), not on the device directly:
that's `usbsas-fs2dev`'s job. The image has the size of the destination device,
an MBR with a single partition, and a `FAT`, `exFAT` or `NTFS` file system
(implemented in `usbsas-fsrw`).

While writing, files2fs records which sectors were written in a bit vector.
fs2dev then only writes those sectors on the device, instead of the whole
image.

It is also used to write disk images: in raw mode, data is appended to the file
as is.

## Usage

```
usbsas-files2fs <FS_PATH>
```

`FS_PATH` must already exist, it is created by usbsas.

## Protocol

`writedst.proto3`, requester: `usbsas-usbsas`.

File system mode:

1. `Init { dev_size, fstype }`: write the MBR and create the file system.
2. For each file: `NewFile`, `WriteFile` requests (sequential, no sparse
   writes), `EndFile`. Directories must be created before their content.
3. `Close`: unmount the file system.
4. `BitVec` until the response has `last` set: get the bit vector of written
   sectors (one bit per sector, sent in chunks). The first sectors (MBR and
   gap before the partition) are always marked written so that an old
   partition table on the device is erased.
5. `End`.

Raw mode (disk image): `WriteRaw`, then `WriteData` requests, then `End`.

## Sandbox

seccomp, the image being the only file the process can access.
