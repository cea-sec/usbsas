# usbsas-local2files

usbsas process reading files from a local directory, used as a transfer source
(`[[src_local_dirs]]` in the configuration).

It implements the read side of the `files.proto3` protocol, like
`usbsas-scsi2files` does for USB devices, so usbsas can browse and copy files
from either.

## Usage

```
usbsas-local2files [-c CONFIG_PATH]
```

## Protocol

`files.proto3`, requester: `usbsas-usbsas`.

1. `OpenLocalDir { path }`: `path` must be one of the configured
   `src_local_dirs`.
2. `GetAttr`, `ReadDir`, `ReadFile` (paths relative to the opened directory),
   until `End`.

Only regular files and directories are listed, other file types are ignored.

## Sandbox

The configuration is read, then the process enters seccomp before parsing it.
landlock then restricts file system access to the configured directories (read
only). If no local directory is configured, no file can be accessed and the
process waits for `End`.
