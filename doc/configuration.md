# usbsas configuration

This file describes how to configure the system for usbsas, the configuration
file, the remote servers and the logs. See the [kiosk documentation](./kiosk.md)
for a complete deployment.

* [System configuration](#system)
* [Configuration file](#configfile)
    * [General](#general)
    * [Sources & destinations](#srcdst)
    * [Analyzer](#analyzer)
    * [Reports](#reports)
    * [Filters](#filters)
    * [Maximum depth](#maxdepth)
    * [USB port assignment](#usbport)
    * [Graphical interface](#graphicalinterface)
    * [Post copy command](#postcopy)
* [User identification](#identifier)
* [Remote servers](#remote)
* [Services and logs](#services)
* [Administration tools](#tools)
* [Running from the build directory](#devrun)

## <a name="system">System configuration</a>

The Debian packages do all of the following when they are installed (see [kiosk
deployment](./kiosk.md#packages)). This section describes what they do, for
other distributions or manual installations.

### Kernel modules

usbsas handles USB mass storage devices in user space: the kernel must not
handle them itself with the `usb_storage` and `uas` modules, which would be
loaded automatically when a device is plugged. Either use a kernel built
without `CONFIG_USB_STORAGE` and `CONFIG_USB_UAS`, or prevent these modules from
loading:

```shell
$ cat << EOF | sudo tee /etc/modprobe.d/usbsas.conf
install usb_storage /bin/false
install uas /bin/false
blacklist usb_storage
blacklist uas
EOF
$ sudo rmmod uas usb_storage
$ sudo update-initramfs -u
```

The same goes for the kernel HID drivers if the user space HID driver
(`usbsas-hid`) is used.

### <a name="usbpermissions">USB permissions</a>

usbsas runs as an unprivileged user (`usbsas` in the packages) which needs
read and write access to USB mass storage devices. A udev rule gives it the
ownership of these devices when they are plugged,
`/etc/udev/rules.d/99-usbsas.rules`:

```
ACTION=="add", SUBSYSTEM=="usb", ENV{ID_USB_INTERFACES}=="*:080[26]50:*", MODE="0660", OWNER="usbsas"
```

It matches devices with a mass storage interface (class `08`) using bulk only
transport (`50`) and the SCSI (`06`) or MMC-2 (`02`, optical disk readers)
command set.

Running usbsas as root isn't needed and isn't recommended.

### Working directory

usbsas writes its working files (archives and file system image of the current
transfer) in `out_directory` (see [General](#general)). It must be writable by
the `usbsas` user and large enough: when files are analyzed or written to a USB
device, usbsas needs at least twice the size of the transferred files. It
should not be readable by other users.

## <a name="configfile">Configuration file</a>

The configuration file uses the [TOML](https://toml.io/en/) format. It is
`/etc/usbsas/config.toml` by default and every usbsas program accepts `-c
<path>` to use another one. The server, the client, the HID driver and the
tools read the same file, each one using the sections it needs. The server reads
it at the beginning of each session, so a change applies to the next session;
the client only reads it when it starts, so restart the `usbsas-client` service
after changing the [graphical interface](#graphicalinterface) or
[report](#reports) options.

A commented example is available [here](../config.example.toml).

### <a name="general">General</a>

```toml
out_directory = "/tmp/usbsas"
keep_tmp_files = false
```

| Option | Default | Description |
|--------|---------|-------------|
| `out_directory` | `/tmp/usbsas` | Directory of the working files of the sessions |
| `keep_tmp_files` | `false` | Keep the working files after a session (for debugging), instead of removing them |

### <a name="srcdst">Sources & destinations</a>

USB devices are always available as sources and destinations (unless
restricted with [USB port assignment](#usbport)). Other sources and
destinations can be added. `description` is the name displayed to the user and
`longdescr` a longer description.

#### Network destinations

Upload the transferred files (in a tar archive) to a remote server. Several
network destinations can be configured.

```toml
[[networks]]
description = "Network XXX"
longdescr = "Send files on network XXX"
url = "https://upload.your.domain/api/uploadbundle"
krb_service_name = "HTTP@upload.your.domain"
```

`krb_service_name` is optional, it enables Kerberos mutual authentication with
the server (a ticket must be acquired by other means). See [Remote
servers](#remote) for the API the server must implement.

#### Network source

Download an archive prepared for the user from a remote server, and write its
files to a USB device. The user is asked for a code (PIN) identifying the
archive.

```toml
[source_network]
description = "Source Network XXX"
longdescr = "Export files from network XXX"
url = "https://download.your.domain/api/downloadbundle"
#krb_service_name = "HTTP@download.your.domain"
```

#### Local directory sources

Let users copy files from local directories (for instance a network share
mounted on the station) to a USB device. They must be readable by the `usbsas`
user.

```toml
[[src_local_dirs]]
path = "/path/to/dir"
description = "Local export"
longdescr = "Export files from local directory XXX"
```

#### <a name="cmdst">Command destination</a>

Execute a command on the transferred files instead of writing them to a device
or uploading them. `%SOURCE_FILE%` in the arguments is replaced with the path of
the tar archive containing the files.

```toml
[command]
description = "Send files with rsync"
longdescr = "Send files on HOST"
command_bin = "/usr/bin/rsync"
command_args = [
    "%SOURCE_FILE%",
    "USER@HOST:PATH"
]
```

**Warning**: the process executing the command isn't sandboxed and the
archive contains files coming from the source device. Make sure the command
handles untrusted data safely.

### <a name="analyzer">Analyzer</a>

Send files to a remote analyzer (antivirus) before writing them to the
destination. Files reported as dirty aren't transferred and are listed in the
report. Analysis only happens when the source is a USB device, and can be
enabled per destination type.

```toml
[analyzer]
url = "https://analyzer.your.domain/api/scanbundle"
#krb_service_name = "HTTP@analyzer.your.domain"
analyze_usb = true
analyze_net = false
analyze_cmd = true
poll_timeout = 1800
```

| Option | Default | Description |
|--------|---------|-------------|
| `url` | mandatory | URL of the analyzer, see [Remote servers](#remote) |
| `krb_service_name` | none | Kerberos service name, enables Kerberos authentication |
| `analyze_usb` | mandatory | Analyze files transferred to a USB device |
| `analyze_net` | mandatory | Analyze files uploaded to a network destination |
| `analyze_cmd` | mandatory | Analyze files given to the command destination |
| `poll_timeout` | `1800` | Maximum time, in seconds, to wait for the result of the analysis once the files are uploaded. If the result isn't available in time, the transfer is aborted. |

### <a name="reports">Reports</a>

A report is produced at the end of each transfer: user, date, station, source
and destination, transferred files with their size and SHA-256, filtered,
rejected and failed files, and the analyzer's report. It is always displayed to
the user, and can also be written as a JSON file:

```toml
[report]
write_dest = true
write_local = "/var/lib/usbsas/reports"
```

| Option | Description |
|--------|-------------|
| `write_dest` | Write the report on the destination (in the file system of a USB device, or in the uploaded archive) |
| `write_local` | Directory where the client saves the reports, on the station. It must be writable by the `usbsas-client` user (the package creates `/var/lib/usbsas/reports`). |

### <a name="filters">Filters</a>

Files can be excluded from transfers based on their path. A file is filtered
if it matches at least one filter. A filter matches if all of its directives
match:

* `exact`: the path is exactly the value,
* `start`: the path starts with the value,
* `end`: the path ends with the value,
* `contain`: the path contains all the values.

Filters are case insensitive (write them in lower case) and tested on the full
path from the root of the source (not the base name). Filtered files are listed
in the report.

```toml
[[filters]]
contain = ["__macosx"]

[[filters]]
start = "lorem ipsum"

[[filters]]
end = ".lnk"
contain = ["whatever"]

[[filters]]
exact = "autorun.inf"

[[filters]]
contain = ["thumbs.db"]
```

### <a name="maxdepth">Maximum depth</a>

Maximum depth of the files of a source, i.e. the number of components of their
path (`/a/b/c.txt` is 3 deep). Deeper files can't be browsed nor transferred,
the user is warned and the directories whose content was skipped are listed in
the report. This protects against sources with very deep directory trees.

```toml
max_depth = 20
```

The default is 20.

### <a name="usbport">USB port assignment</a>

By default, any USB device can be used as a source or a destination, whatever
the port it's plugged in. usbsas can be configured to only accept sources,
destinations and HID devices (mice, touch screens, with
[usbsas-hid](./kiosk.md#debhid)) on specific ports, for instance to use dedicated
and labeled ports on the station:

```toml
[usb_port_accesses]
ports_src = [ [3, 2, 5], [2, 3] ]
ports_dst = [ [4, 3] ]
ports_hid = [ [3, 3] ]
```

A port is designated by the bus number followed by the port numbers at each
level of hubs. In this example, source devices are accepted on port 5 of the
hub connected to port 2 of bus 3, and on port 3 of bus 2. Destination devices
are accepted on port 3 of bus 4, HID devices on port 3 of bus 3. If a list isn't
set, every port is allowed for this role. Devices plugged in other ports are
ignored by usbsas (they aren't disabled though).

The `usbsas-devmonitor` tool (see [Administration tools](#tools)) prints the
port path of devices when they are plugged, `lsusb -t` can also be used. Keep
in mind that USB 3 ports usually also handle USB 2: the same physical port
corresponds to two buses, list both.

### <a name="graphicalinterface">Graphical interface</a>

```toml
lang = "en"
window_title = "USBSAS"
menu_img = "/path/to/image"
```

| Option | Default | Description |
|--------|---------|-------------|
| `lang` | `en` | Default language of the interface, `en` or `fr` (users can change it) |
| `window_title` | `USBSAS` | Title displayed in the menu bar |
| `menu_img` | none | Image (logo) displayed in the menu bar, next to the host name |

### <a name="postcopy">Post copy command</a>

Execute a command after each transfer, for instance to archive transferred
files. `%SOURCE_FILE%` is replaced with the path of the output: the tar archive
for network and command destinations, the file system image for a USB
destination.

```toml
[post_copy]
description = "Archive transfer"
command_bin = "/bin/cp"
command_args = [
    "%SOURCE_FILE%",
    "/usbsas_archive/"
]
```

Like the [command destination](#cmdst), this command isn't sandboxed.

## <a name="identifier">User identification</a>

usbsas requires a user id before a transfer: it is written in the logs and
reports, sent to the analyzer and the remote servers, and used to find the
archive of a network source.

**The provided identifier (`usbsas-identifier`) is a placeholder** that always
returns the same id (`Tartempion`). A deployment that needs to know who uses
the station must replace it with an implementation identifying users (badge,
smart card...) and answering the same protocol (`identifier.proto3`).

## <a name="remote">Remote servers</a>

usbsas talks HTTP(S) to remote servers for the analyzer, network destinations
and the network source. `USER` is the user id.

| Use | Requests |
|-----|----------|
| Analyzer (`[analyzer] url`) | `POST URL/USER` with the tar archive to analyze, the server answers an analysis id (`{"id": "ID", "status": "uploaded"}`). usbsas then polls `GET URL/USER/ID` every second until the status is `scanned` (or `poll_timeout` is reached). |
| Network destination (`[[networks]] url`) | `POST URL/USER` with the tar archive |
| Network source (`[source_network] url`) | `HEAD URL/USER/PIN` must give the size of the archive in the `X-Uncompressed-Content-Length` header, then `GET URL/USER/PIN` downloads it (gzip encoding is supported) |

Archives contain the files under a `data/` directory, and a `config.json` file
describing the transfer.

When the analysis is done, the analyzer answers its report, with the status of
each file (paths relative to `data/`):

```json
{
  "id": "ID",
  "status": "scanned",
  "version": 2,
  "files": {
    "dir/file.txt": { "status": "CLEAN" },
    "eicar.com":    { "status": "DIRTY" }
  },
  "antivirus": {
    "ClamAV": {
      "version": "ClamAV 1.4.3",
      "database_version": "27800",
      "database_timestamp": "Tue Oct 21 08:23:45 2026"
    }
  }
}
```

Only files reported `CLEAN` are transferred.

### Analyzer server

`usbsas-analyzer-server` implements the three APIs above, with
[ClamAV](https://www.clamav.net/) for the analysis. It's the reference
implementation, used by the integration tests: **it is a demo**, production
use isn't recommended. Its options, API details and limits (e.g. ClamAV's
maximum file size) are documented in its
[README](../usbsas-analyzer-server/README.md).

The `usbsas-analyzer-server` package installs it with a systemd service
listening on `127.0.0.1:8042`. It starts its own `clamd`, so the system
`clamav-daemon` service should be disabled, and `clamav-freshclam` needs to
reach the Internet (or a mirror) to update the virus database:

```shell
$ sudo systemctl disable --now clamav-daemon.service
$ sudo systemctl enable --now usbsas-analyzer-server.service
```

## <a name="services">Services and logs</a>

The packages install these systemd services (none is enabled by default):

| Service | Package | User | Description |
|---------|---------|------|-------------|
| `usbsas-server` | `usbsas-server` | `usbsas` | The server (`usbsas-usbsas -s`), restarted after each session |
| `usbsas-client` | `usbsas-kiosk` | `usbsas-client` | The graphical client, full screen in `cage` on tty2 |
| `hid-dealer` | `usbsas-hid` | `usbsas-hid` | The user space HID driver |
| `usbsas-analyzer-server` | `usbsas-analyzer-server` | `usbsas` | The demo analyzer server |

### Logs

All usbsas processes log on stderr, collected by journald with the services
above:

```shell
$ journalctl -u usbsas-server
```

The log level is set with the `RUST_LOG` environment variable (`info` in the
services, see [env_logger](https://docs.rs/env_logger)). At the `info` level,
the server logs what users do and what is transferred: user identification,
selected source and destination, browsing and selected files, filtered and
rejected files, each file read from the source with its size and SHA-256, the
analysis results and the outcome of the transfer. Each session has an id
(`USBSAS_SESSION_ID`), also written in the reports.

Built with the `usbsas-utils/log-json` feature (see [build](./build.md#options)),
processes log one JSON object per line, with the session id in a `transfer_id`
field, which eases sending logs to a central log server.

## <a name="tools">Administration tools</a>

The `usbsas-tools` package provides standalone command line tools built with
usbsas processes. Like usbsas, they don't use the kernel mass storage drivers
and are sandboxed. Devices are designated by their bus and device numbers
(given by `lsusb`). Each tool describes its options with `--help`.

| Tool | Description |
|------|-------------|
| `usbsas-devmonitor` | Print plugged and unplugged USB devices with their port path, to configure [USB port assignment](#usbport) |
| `usbsas-imager` | Copy a whole USB device to a file or to stdout, like `dd` |
| `usbsas-fswriter` | Write a file system image to a USB device, like `dd` |
| `usbsas-fuse-mount` | Mount a partition of a USB device, read only, with FUSE |
| `usbsas-net` | Test the remote servers: upload, analyze or download an archive |

Examples:

```shell
$ usbsas-imager -o /tmp/device.img 1 12
$ usbsas-fuse-mount -n 1 1 12 /mnt
```

Disk images made from the interface (Tools menu) are saved in `out_directory`
as `usbsas_imgdisk_<session id>.img`.

## <a name="devrun">Running from the build directory</a>

To try usbsas without installing the packages (after
[building](./build.md)), configure the [kernel modules](#system) and [USB
permissions](#usbpermissions) for your user, then start the server and the
client:

```shell
$ ./target/release/usbsas-usbsas -c config.example.toml -s /tmp/usbsas-socket
$ ./target/release/usbsas-client -c config.example.toml -s /tmp/usbsas-socket/usbsas.sock
```

The server listens on a Unix socket created in the given directory
(`/run/usbsas` by default). Instead of the graphical client, it can be driven by
a Python script, see the [Python module](../python/README.md).
