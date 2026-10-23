# usbsas-utils

Constants and small helpers shared by every usbsas crate.

## Constants

| Constant              | Value                       | Use                                                    |
|-----------------------|-----------------------------|--------------------------------------------------------|
| `INPUT_PIPE_FD_VAR`   | `INPUT_PIPE_FD`             | env var holding the fd a child reads its requests from |
| `OUTPUT_PIPE_FD_VAR`  | `OUTPUT_PIPE_FD`            | env var holding the fd a child writes its responses to |
| `MAX_PROTO_MESS_SIZE` | 32 MiB                      | largest protobuf message accepted (`usbsas-comm`)      |
| `READ_FILE_MAX_SIZE`  | 10 MiB                      | largest chunk of file / sectors read in one request    |
| `SECTOR_SIZE`         | 512                         | sector size of the file systems written by usbsas      |
| `TAR_BLOCK_SIZE`      | 512                         | tar block size                                         |
| `TAR_DATA_DIR`        | `data`                      | directory containing the files in usbsas tar archives  |
| `DEFAULT_SOCKET_DIR`  | `/run/usbsas`               | default directory of the server socket                 |
| `USBSAS_BIN_PATH`     | build time, see below       | directory of the usbsas executables                    |
| `USBSAS_CONFIG`       | build time, see below       | default configuration file                             |

Two constants are set when the crate is built, from environment variables of
the same name:

- `USBSAS_BIN_PATH`: where processes look for the executables of their
  children. Defaults to the cargo target directory of the current profile
  (e.g. `target/release/`), packages use `/usr/libexec/`.
- `USBSAS_CONFIG`: default configuration file path, `/etc/usbsas/config.toml`
  if unset.

```shell
$ USBSAS_BIN_PATH=/usr/libexec/ USBSAS_CONFIG=/etc/usbsas/config.toml cargo build --release
```

## Modules

- `clap`: `new_usbsas_cmd()` and the `UsbsasClap` trait adding the arguments
  shared by usbsas binaries (`-c/--config`, `-s/--socket`, tar and fs output
  paths).
- `log`: `init_logger()`, initializes `env_logger` (level from `RUST_LOG`,
  `info` by default).

The `formathex!` and `dbgprinthex!` macros format / print byte arrays in
hexadecimal for debugging.

## Features

- `log-json`: `init_logger()` writes one JSON object per line, with the
  timestamp, level, target, message and session id (`transfer_id`, from
  `USBSAS_SESSION_ID`). No crate enables it by default, use
  `cargo build --features usbsas-utils/log-json`.
