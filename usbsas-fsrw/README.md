# usbsas-fsrw

File systems readers and writers used by usbsas.

| File system | Read (`FSRead`)                        | Write (`FSWrite`)                      |
|-------------|----------------------------------------|----------------------------------------|
| FAT12/16/32 | `fat::FatReader` ([fatfs])             | `fat::FatWriter` ([fatfs])             |
| exFAT       | `ff::ExFatFsReader` ([FatFs], C)       | `ff::ExFatFsWriter` ([FatFs], C)       |
| NTFS        | `ntfs::NTFS` ([ntfs])                  | `ntfs::NTFS3G` ([ntfs-3g], C)          |
| ext4        | `ext4fs::Ext4` ([ext4-view])           |                                        |
| ISO9660     | `iso9660::Iso9660` ([hadris-iso])      |                                        |

Readers are used by `usbsas-scsi2files` to parse the file system of the source
device (from sectors read by dev2scsi). Writers are used by `usbsas-files2fs` to
create a new file system in an image written to the destination device. Writers
only create new file systems, they can't modify existing ones.

Both run in sandboxed processes: a bug in a (C) file system implementation
parsing a malicious device stays confined in scsi2files.

## Traits

- `FSRead<T>`: mount a file system from a `Read + Seek` stream, then
  `get_attr()`, `read_dir()` and `read_file()` by path. `unmount_fs()` gives the
  stream back.
- `FSWrite<T>`: `mkfs()` a new file system on a `Read + Write + Seek` stream of
  `sector_count` sectors, then `newdir()`, `newfile()` (returns a writer),
  `settimestamp()` and `removefile()`. `unmount_fs()` flushes the file system and
  gives the stream back.

## C libraries

Two internal crates build and bind vendored C libraries (with `clang` and
`bindgen`):

- `ff/`: [FatFs] (custom BSD-style license, see `ff/src/ff/LICENSE.txt`), for
  exFAT,
- `ntfs3g/`: [ntfs-3g] (GPLv2, see `ntfs3g/src/ntfs-3g/COPYING`), only used to
  create and write new NTFS file systems.

[fatfs]: https://crates.io/crates/fatfs
[FatFs]: http://elm-chan.org/fsw/ff/
[ntfs]: https://crates.io/crates/ntfs
[ntfs-3g]: https://github.com/tuxera/ntfs-3g
[ext4-view]: https://crates.io/crates/ext4-view
[hadris-iso]: https://crates.io/crates/hadris-iso
