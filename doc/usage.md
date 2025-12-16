# usbsas features & usage

* [Graphical User Interface](#gui)
* [Analyzer server](#asrv)
* [Upload & download from remote server](#updown)
* [HID](#hid)
* [Python scripting](#python)
* [Tools](#tools)
* [Reading disk images](#diskimage)

## <a name="gui">Graphical User Interface</a>

The graphical client has been developed with the [kiosk](./kiosk.md) station in
mind, e.g. simple full screen window that can't be closed or minimized to
interact with other components of the system.

It allows transferring files from USB to USB or remote host, wiping a USB device and
making image of a USB device.

After [building](./build.md), start `usbsas` and the `client`:

The GUI interacts with usbsas via a Unix domain socket so usbsas must be started
with `-s` option.

```shell
$ ./target/release/usbsas-usbsas -s
```

```shell
$ ./target/release/usbsas-client
```

[Here](./ui_usage.md) are some screenshots of the interface.


## <a name="asrv">Analyzer server</a>

analyzer-server included in this project is mainly used for tests and given as
example, production use is not recommended. Files are analyzed with [Clam
AntiVirus](https://www.clamav.net/)

Scanning files is enabled or disabled in the [configuration
file](./configuration.md#analyzer).

The archive containing files to analyze is first POSTed to "URL/[user_id]", the
server should respond a unique analysis identifier. The analyzer process will
then poll the remote server on "URL/[user_id]/[analyze_id]". The expected answer
from the server is a JSON containing a status string, when the analysis is done,
status should be "scanned" and the JSON response should include sanity status
for all files, for example:

```json
{
  "status": "scanned",
  "id": "f092fb9a883b439eaf5c6e75bcdc646e",
  "files": {
    "SCSI Commands Reference Manual.pdf": {
      "status": "CLEAN",
      "sha256": "XXX"
    },
    "directories/a/man_rustc.txt": {
      "status": "CLEAN",
      "sha256": "XXX"
    },
    "eicar.com": {
      "status": "DIRTY",
      "sha256": "XXX"
    }
  },
  "antivirus": {
    "ClamAV": {
        "version": "XXX",
        "database_version": "XXX",
        "database_timestamp": "XXX"
    }
  }
}
```

This report can be written on the destination device (if enabled in the
configuration file).

It supports Kerberos mutual authentication if compiled with the `authkrb`
feature (enabled by default) and a service name is present in the configuration
file.

## <a name="updown">Upload & download from remote server</a>

Files read from a USB device can be sent to a remote server (in a tar archive).
The archive is sent by a HTTP POST request at the URL:
`https://HOST:PORT/api/uploadbundle/USERID`

Files can also be downloaded from the remote server to be copied in a
destination USB device.
If this source is selected, the user is asked for a pin code and the archive
will be downloaded from this URL:
`https://HOST:PORT/api/downloadbundle/USERID/PIN`


See the [configuration file](./configuration.md#srcdst).

## HID

In order to protect against BadUSB devices, a minimal userland HID driver has
also been implemented (`hid` kernel module can be disabled / removed). It only
supports mouse left click (enough for selecting files to transfer in the UI),
keystrokes sent by a malicious device (or any keyboard) won't be handled by the
system.

More details [here](./kiosk.md#debhid).


## <a name="python">Python scripting</a>

Instead of using the GUI, usbsas can be controlled by a Python script with a
module that ease protobuf communication. [Here](../python/README.md) is an example.

## Tools

Collection of tools built with usbsas components to show how it can be used as a
framework.

### Mount a device

`usbsas-fuse-mount` is a standalone tool to mount a USB Mass storage device
(read-only) with fuse.

```shell
$ usbsas-fuse-mount --help

Mount a (fuse) filesystem with usbsas

USAGE:
    usbsas-fuse-mount [OPTIONS] <busnum> <devnum> <mountpoint>

ARGS:
    <busnum>        Bus number of the device to mount
    <devnum>        Dev number of the device to mount
    <mountpoint>    Path to mount the device

OPTIONS:
    -h, --help                  Print help information
    -n, --part-num <PARTNUM>    Partition number to mount [default: 1]
    -V, --version               Print version information
```

### Imager

Standalone tool to make an image of a USB Mass Storage device (like `dd`).
```
$ usbsas-imager  --help

Usage: usbsas-imager [OPTIONS] <BUSNUM> <DEVNUM>

Arguments:
  <BUSNUM>  Bus number of the output device
  <DEVNUM>  Device number of the output device

Options:
  -c, --config <config>  Path of the configuration file [default: /etc/usbsas/config.toml]
  -o, --output <FILE>    Path of the output file
  -O, --stdout           Output to stdout
  -h, --help             Print help
  -V, --version          Print version
```

### Filesystem writer
Standalone tool to write a filesystem on a USB Mass Storage device (like `dd)`.
```
$ usbsas-fswriter --help

Usage: usbsas-fswriter <FILE> <BUSNUM> <DEVNUM>

Arguments:
  <FILE>    Path of the input filesystem
  <BUSNUM>  Bus number of the output device
  <DEVNUM>  Device number of the output device

Options:
  -h, --help     Print help
  -V, --version  Print version
```

### <a name="diskimage">Reading disk images / dumped devices</a>

The `mock` feature used for the integration tests also allows using usbsas with
_fake_ (file-backed) USB devices. After building with this feature (`cargo build --features mock`),
2 variables can be set:

```shell
$ export USBSAS_MOCK_IN_DEV=/tmp/mock_input_dev.img
$ export USBSAS_MOCK_OUT_DEV=/tmp/mock_output_dev.img
$ usbsas-usbsas -s
```

usbsas won't list real plugged devices but those in these variables instead.
