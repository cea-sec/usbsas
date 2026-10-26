# usbsas-fs2dev

usbsas process writing to the destination USB device. It's the only process
with write access to it.

It can:

- copy the file system image created by `usbsas-files2fs` on the device,
  writing only the sectors listed in the bit vector received from usbsas,
- wipe the device (fill it with zeros).

Writes are done with SCSI write commands through `usbsas-mass-storage` (USB bulk
transfers with libusb on `/dev/bus/usb/BBB/DDD`), without kernel drivers. They
are at most 240 sectors long since some devices don't support larger ones.

## Usage

```
usbsas-fs2dev <FS_PATH>
```

It is spawned locked by `usbsas-usbsas` and waits for its unlock value: the bus
number (low 32 bits) and device number (high 32 bits) of the destination
device, or `0` if no USB device will be written this session.

## Protocol

`fs2dev.proto3`, requester: `usbsas-usbsas`.

| Request      | Effect                                                         |
|--------------|----------------------------------------------------------------|
| `DevSize`    | size of the device in bytes                                    |
| `Wipe`       | fill the device with zeros, `Status` responses until done      |
| `LoadBitVec` | load the bit vector of sectors to write (in chunks, until `last`) |
| `WriteFs`    | write the image to the device, `Status` responses until done (after `LoadBitVec`) |
| `End`        | exit                                                           |

## Sandbox

seccomp, with access only to the image and the device opened at unlock.

## Features

- `mock`: write to the file given by `USBSAS_MOCK_OUT_DEV` instead of a real
  device (see `usbsas-mock`), used by integration tests.
