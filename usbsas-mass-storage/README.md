# usbsas-mass-storage

Access to USB mass storage devices, as readable (and seekable) streams.

It provides two structs, used on each side of the process boundary between
the code accessing the source device and the code parsing its file systems:

- `MassStorage`, used by `usbsas-dev2scsi` and `usbsas-fs2dev`: talks to the
  device with SCSI commands over USB bulk transfers (`usbsas-scsi`), with libusb
  on an already opened `/dev/bus/usb/BBB/DDD` file. The kernel mass storage
  drivers aren't involved. Writes are checked by reading back the last sector
  written.
- `MassStorageComm`, used by `usbsas-scsi2files`: implements `Read`, `Seek` and
  `ReadAt` on top of `ReadSectors` requests sent to dev2scsi, relative to the
  opened partition, so that file system readers (`usbsas-fsrw`) can use it like a
  file. Small reads (at most `MAX_SECTORS_COUNT_CACHE` sectors, i.e. file system
  metadata) are cached.

Only devices with a block size of 512, 1024, 2048 or 4096 bytes are supported.
