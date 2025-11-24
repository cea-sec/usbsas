# usbsas configuration

## System requirements

### Kernel modules

One of the main feature of usbsas is to work in user space, thus the Linux
kernel must not have `usb_storage` and `uas` modules. Either compile a kernel
with `CONFIG_USB_STORAGE` and `CONFIG_USB_UAS` unset or at least prevent these
modules to load because if present, they will be loaded automatically when a USB
device is plugged.

```shell
$ cat << EOF > /etc/modprobe.d/usbsas.conf
install usb_storage /bin/false
blacklist usb_storage
install uas /bin/false
blacklist uas
EOF
$ rmmod usb_storage
$ rmmod uas
$ depmod
```

### USB permissions

`usbsas` needs R/W permissions on USB devices, multiple options:

- create a specific user, give it ownership of USB devices with a `udev` rule
  and run usbsas with this user.
- OR give ownership of the device to your user: `$ chown user /dev/bus/usb/XXX/YYY`
- OR run it as root (not recommended)


udev rule `/etc/udev/rules.d/99-usbsas.rules`:
```
ACTION=="add", SUBSYSTEM=="usb", ENV{ID_USB_INTERFACES}=="*:080650:*", MODE="0660", OWNER="usbsas"
```

This rule will give ownership of the device to user `usbsas` if the device has
an interface with the class mass storage (0x80), SCSI command set (0x06) and
Bulk transport mode (0x50).


## Configuration file

usbsas's configuration file uses the [TOML](https://toml.io/en/) file format and
is located in `/etc/usbsas/config.toml` by default. usbsas client, server and
tools can be started with the `-c <path_to_conf>` to override it.

An example is available [here](../config.example.toml).

* [Sources & Destinations](#srcdst)
* [Analyzer](#analyzer)
* [USB port assignment](#usbport)
* [Filters](#filters)
* [Graphical interface](#graphicalinterface)
* [Misc](#misc)


### <a name="srcdst">Sources & Destinations</a>

#### Network destination

Send files (in a tar) to a remote network.

Optional `krb_service_name` can be specified if Kerberos authentication is used (a
ticket must be acquired by other means).

```toml
[[networks]]
description = "Network XXX"
longdescr = "Send files on network XXX"
url = "http://127.0.0.1:8042/api/uploadbundle"
krb_service_name = "HTTP@your.domain"
```

Multiple `networks` can be added.

#### <a name="cmdst">Command destination</a>

Execute a command to process input files, `%SOURCE_FILE%` is replaced with the
path of the archived (tar) files.

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

Warning: The process executing the command is not sandboxed


#### Source network

Download files from a remote host, to write them on a USB device.

```toml
[source_network]
description = "Source Network XXX"
longdescr = "Export files from network XXX"
url = "http://127.0.0.1:8042/api/downloadbundle"
#krb_service_name = "HTTP@your.domain"
```

### Analyzer

Configure remote analyzer server address and whether to analyze depending on the
destination (analysis only happens if source is `usb`).

```toml
[analyzer]
url = "http://127.0.0.1:8042/api/scanbundle"
#krb_service_name = "HTTP@your.domain"
analyze_usb = true
analyze_net = false
analyze_cmd = true
```

Analyzer report can be written on the destination device and locally

```toml
[report]
write_dest = true
write_local = "/var/lib/usbsas/reports"
```

### <a name = "usbport">USB port assignment</a>

usbsas can be configured to only handle devices plugged on specific USB ports.

Example:

```toml
[usb_port_accesses]
ports_src = [ [3, 2, 5], [2, 3] ]
ports_dst = [ [4, 3] ]
```

Here source devices can only be plugged on the port 5 of the hub that is
connected on the port 2 of the bus 3 and on the port 3 of the bus 2. Destination
devices can only be plugged on the port 3 of the bus 4.

`$ lsusb -t` can be used to check USB topology.


This only means usbsas will ignore devices plugged in other ports, they won't be
disabled however. Same port can be allowed for both source and destination.


### Filters

Files can be filtered from source devices based on their names.
A file is filtered is its name matches a filter, a filter matches if all of its
directives match.

Available directives are:

* `exact`: filename is exactly the same as *value*
* `start`: filename starts with *value*
* `end`: filename ends with *value*
* `contain`: filename contains *value*

Filters are case insensitive and tested on full path from the source device (not
the basename).

Example:

```toml
[[filters]]
contain = ["__macosx"]

[[filters]]
start = ["lorem ipsum"]

[[filters]]
end = ".lnk"
contain = "whatever"

[[filters]]
exact = ["autorun.inf"]

[[filters]]
contain = ["thumbs.db"]
```

### <a name="graphicalinterface">Graphical interface</a>

Change language, can either be English (`en`) or French (`fr`).

```toml
lang = "en"
```

Custom window title and menu image (next to hostname)

```toml
window_title = "USBSAS"
menu_img = "/path/to/image"
```

### Misc

#### Temporary files

Where temporary files are stored and whether to keep them between transfers

```toml
out_directory = "/tmp/usbsas"
keep_tmp_files = false
```

#### Post copy

Execute a command after a transfer, similar to [command destination](#cmdst).

```toml
[post_copy]
description = "Archive transfer"
command_bin = "/bin/cp"
command_args = [
	"%SOURCE_FILE%",
	"/usbsas_archive/"
]
```
