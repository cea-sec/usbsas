# usbsas-mbr

Minimal MBR (master boot record) partition table parser and writer.

Used by `usbsas-files2fs` to write the partition table of the file system image
created for a destination device: a single partition starting at sector
`SECTOR_START` (2048, the usual 1 MiB alignment).

- `parse_partition_table()`: parse the 4 primary entries of an MBR (empty ones
  are skipped), checking its `0x55AA` signature,
- `MbrPartitionEntry::new()`: build an entry from a type and an LBA range (CHS
  fields are computed from the LBA, with the conventional 255 heads / 63
  sectors geometry),
- `write_partition()`: write an entry at the current position of a writer.
