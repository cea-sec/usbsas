# usbsas-scsi

SCSI over USB Mass Storage Bulk-Only Transport (BOT), in user space.

usbsas doesn't use the kernel `usb_storage` / `uas` drivers: the processes
accessing USB devices (`usbsas-dev2scsi` and `usbsas-fs2dev`, through
`usbsas-mass-storage`) send SCSI commands themselves over the bulk endpoints of
the device with libusb (`rusb`).

`ScsiUsb`:

- `init_mass_storage()`: get the number of logical units, find a ready
  direct-access (disk) or CD/DVD unit (INQUIRY, TEST UNIT READY) and read its
  capacity. Large devices are driven with 16-byte commands (READ CAPACITY(16),
  READ(16), WRITE(16)).
- `read_sectors()` / `read_blocks()` / `write_blocks()`: read and write blocks,
  splitting large reads in commands of at most `MAX_SECTORS_PER_TRANSFER`
  blocks.
- failed commands are reported with their sense data (REQUEST SENSE), timeouts
  and stalls are recovered when possible (endpoint clear, Bulk-Only reset).

The USB handle is abstracted behind the `UsbTransport` trait (implemented for
`rusb::DeviceHandle`), so the protocol is tested against a simulated device
without hardware:

```shell
$ cargo test -p usbsas-scsi
```
