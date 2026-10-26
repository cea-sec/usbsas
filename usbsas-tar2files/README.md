# usbsas-tar2files

usbsas process reading files from the session tar archive.

During a transfer, files are first copied from the source into a tar (see
`usbsas-files2tar`). tar2files reads them back to write them on the destination.
It also lists the content of an archive downloaded from a network source.

Only the entries under `data/` (the transferred files) and `config.json` (the
report of the source) are exposed, other entries and paths containing `..` are
ignored.

## Usage

```
usbsas-tar2files <TAR_PATH>
```

It is spawned locked by `usbsas-usbsas` and waits for its unlock value: `1` to
open `TAR_PATH`, anything else to wait for `End` without opening it.

## Protocol

`files.proto3`, requester: `usbsas-usbsas`.

| Request    | Response                                                  |
|------------|-----------------------------------------------------------|
| `GetAttr`  | type, size and timestamp of a file                        |
| `ReadDir`  | entries of a directory                                    |
| `ReadFile` | part of a file (at most `READ_FILE_MAX_SIZE` bytes)       |
| `End`      | exit                                                      |

The tar headers are read once when the process is unlocked, then files are read
directly at their offset in the archive.

## Sandbox

seccomp, the tar being the only file the process can read.
