# usbsas-usbsas

The usbsas server: the parent process that orchestrates every other usbsas
process.

Applications (the [GUI](../usbsas-client), the [Python module](../python), the
integration tests...) never talk to USB devices or file systems directly. They
send requests (defined in `usbsas-proto/proto/usbsas.proto`) to `usbsas-usbsas`,
which spawns sandboxed child processes and drives them to list devices, read
files, analyze them, write them to a destination, wipe or image a device. See
the [architecture documentation](../doc/architecture.md) for an overview.

## Usage

`usbsas-usbsas` handles a **single session** (one transfer, wipe or disk image)
then exits; all children are restarted for the next session. In production it
is run by systemd with `Restart=always`.

It can be reached in two ways:

- **Unix socket** (`-s [DIR]`): the server creates `DIR/usbsas.sock` (default
  `/run/usbsas`), accepts one connection and serves it. This is what the GUI
  uses.

  ```shell
  $ ./target/release/usbsas-usbsas -s
  ```

- **Pipes** (no `-s`): the server is spawned by its client (with
  `usbsas-process`) and communicates through the pipe file descriptors given in
  its environment.

Options:

- `-c, --config`: usbsas configuration file (default `/etc/usbsas/config.toml`)
- `-s, --socket`:  listen on a Unix socket in `DIR` (default `/run/usbsas`)

Log level is set with `RUST_LOG`. The `USBSAS_SESSION_ID` environment variable
sets the session id (a random one is generated if unset); it names the
session's output files and is used as the transfer id in reports.

## What it does

On startup, the server:

1. reads the [configuration](../doc/configuration.md),
2. creates the session's working files in `out_directory`:
   - `usbsas_<id>.tar`: files read from the source
   - `usbsas_<id>_clean.tar`: files that passed analysis, for network /
     command destinations
   - `usbsas_<id>.img`: filesystem built for a USB destination (or disk image)
3. spawns its children (binaries looked up in `USBSAS_BIN_PATH`, set at build
   time)
4. sandboxes itself with seccomp: it can only talk to its children and its
   client and remove files in `out_directory`
5. runs its state machine until the client sends `End`.

Working files are removed when the server exits unless `keep_tmp_files` is set.

### Children

- `usbdev`: lists plugged USB mass storage devices
- `identifier`: gets the id of the user
- `scsi2files` : reads files from the source USB device (spawns `dev2scsi`)
- `local2files`: reads files from a local directory source
- `downloader`: downloads an archive from a remote source
- `files2tar`: writes the source files into `usbsas_<id>.tar`
- `tar2files`: reads files back from `usbsas_<id>.tar`
- `analyzer`: sends the tar to a remote analyzer (antivirus)
- `files2fs`: writes files into a new file system (`usbsas_<id>.img`)
- `files2cleantar`: `files2tar` writing `usbsas_<id>_clean.tar`
- `fs2dev`: writes the file system image to the destination USB device, wipes devices
- `uploader`: uploads the tar to a remote server
- `cmdexec`: executes the configured command / post copy command

Some children (`fs2dev`, `tar2files`, `uploader`, `cmdexec`) start *locked*:
they wait for a 64 bit value before opening their resources and sandboxing
themselves, since what they need (destination device, which tar to use) is only
known during the session. `0` tells them they won't be used.

### State machine

The server is a state machine (`src/states.rs`), each state accepting a given
set of requests:

```
Init (Devices, UserId, InitTransfer, Wipe, ImgDisk)
 ├─ Wipe ───────────────────────────────────────────────────────────┐
 ├─ ImgDisk ────────────────────────────────────────────────────────┤
 ├─ OpenSrcUsb (Partitions, OpenPartition) ─> BrowseSrc ─┐          │
 ├─ BrowseSrc (ReadDir, GetAttr, SelectFiles) ───────────┤          │
 └─ DownloadSrc ─────────────────────────────────────────┤          │
                                                         v          │
                FileSelection ─> [Analyze] ─> [WriteDstFile]        │
                                                         v          v
                                                    TransferDst ─> End (Devices, Report) ─> Exit
```

`OpenSrcUsb` is used for a USB source, `BrowseSrc` directly for a local
directory source, and `DownloadSrc` for a network source (the whole archive is
downloaded, there is no browsing). A file transfer then goes through:

1. **FileSelection**: the selected paths are expanded (directories walked),
   filtered with the configured `filters`, and their total size checked against
   the destination and the available space. Files are copied to
   `usbsas_<id>.tar` and hashed (SHA-256), except for a network source whose
   archive was already downloaded there.
2. **Analyze** (if enabled in the `analyzer` configuration, for USB sources
   only): the tar is sent to the analyzer, dirty files are excluded.
3. **WriteDstFile** (USB destination, or any destination after analysis): files
   are read back from the tar and written into a new file system (USB
   destination) or into the clean tar (network / command destination).
4. **TransferDst**: the file system is written to the USB device, the tar (or
   clean tar) is uploaded, or the command is executed.

States waiting for requests also accept `End`, which stops the children and
exits. A transfer with nothing left to copy goes straight to `End`.

Status updates are sent to the client during each step. Once done, the client
can request a `TransferReport`. If an error occurs, the server sends it to the
client and waits for `End`.

## Features

- `integration-tests`: fixed hostname and date in reports so output hashes are
  deterministic, and enables the integration tests.

## Tests

Integration tests (`tests/integration_tests.rs`) run full transfers (USB to
USB, USB to network, wipe...) with mocked devices and a local analyzer server:

```shell
$ cargo build --all --features mock,integration-tests
$ cargo test -p usbsas-usbsas --features integration-tests
```

## Packaging

`cargo deb -p usbsas-usbsas` builds the `usbsas-server` Debian package. It
installs the server and its children in `/usr/libexec`, the example
configuration, a systemd unit running `usbsas-usbsas -s` as the `usbsas` user,
a udev rule giving this user access to USB mass storage devices, and a modprobe
configuration disabling the `usb_storage` and `uas` kernel modules.
