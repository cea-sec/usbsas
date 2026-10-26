# usbsas-files2tar

usbsas process writing files into a tar archive.

`usbsas-usbsas` spawns two of them per session:

- `files2tar` writes the files read from the source into `usbsas_<id>.tar`,
- `files2cleantar` writes the files that passed analysis into
  `usbsas_<id>_clean.tar`, for network and command destinations.

Transferred files are stored under a `data/` directory. Metadata files (e.g.
the `config.json` report) are stored at the root of the archive. Paths
containing `..` are refused.

## Usage

```
usbsas-files2tar <TAR_PATH>
```

`TAR_PATH` must already exist, it is created by usbsas.

## Protocol

`writedst.proto3`, requester: `usbsas-usbsas`.

1. For each file: `NewFile { path, size, ftype, timestamp }`, then
   `WriteFile` requests (directories have no data), then `EndFile`. The data
   written can't exceed the announced size.
2. `Close`: finish the archive.
3. `End`: exit.

## Sandbox

seccomp, the archive being the only file the process can write.
