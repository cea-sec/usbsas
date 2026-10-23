# usbsas-process

Helpers to spawn and manage usbsas child processes.

usbsas is split in many sandboxed processes (see the [architecture
documentation](../doc/architecture.md#communication)). A parent process
(`usbsas-usbsas`, `scsi2files` spawning `dev2scsi`, the analyzer spawning
`jsonparser`, the tools...) uses this crate to start a child and get a typed
`usbsas-comm` channel to send it requests.

## Spawning a child

```rust
use usbsas_comm::ComRqFiles;
use usbsas_process::{ChildMngt, UsbsasChildSpawner};

let mut tar2files = UsbsasChildSpawner::new("usbsas-tar2files")
    .arg("/tmp/usbsas.tar")
    .wait_on_startup()
    .spawn::<ComRqFiles>()?;

tar2files.unlock_with(1)?;
// send requests with tar2files.comm
tar2files.end()?;
tar2files.wait()?;
```

`spawn()`:

- runs the binary from `USBSAS_BIN_PATH` (set when building `usbsas-utils`,
  defaults to the cargo target directory, `/usr/libexec` in packages),
- creates two pipes, one per direction. The child gets its ends via the
  `INPUT_PIPE_FD` and `OUTPUT_PIPE_FD` environment variables (read with
  `Comm::from_env()`), the parent's ends are close-on-exec,
- clears the environment, keeping only a small allow list (`PATH`, `LANG`,
  `TERM`, `RUST_LOG`, `RUST_BACKTRACE`, `KRB5CCNAME`, `USBSAS_SESSION_ID` and the
  `USBSAS_MOCK_*` variables used in tests).

## Locked children

Some children can't open their resources at startup because they depend on the
session (which device to write to, which archive to upload...). They're spawned
with `wait_on_startup()` and block until the parent unlocks them with
`unlock_with(value)`, which writes a raw little endian `u64` on the pipe (before
any protobuf message). The meaning of the value depends on the child; `0`
always means "won't be used" and is sent automatically by `end()` if the child
is still locked.

## Ending a child

`end()` sends the protobuf `End` request and waits for its response, `wait()`
then waits for the process to exit. Children are not killed when their
`UsbsasChild` is dropped.
