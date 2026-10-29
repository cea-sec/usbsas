# usbsas-config

Parser of the usbsas configuration file (TOML, `/etc/usbsas/config.toml` by
default).

The format and every option are described in the [configuration
documentation](../doc/configuration.md), and
[config.example.toml](../config.example.toml) is a commented example.

## Usage

```rust
use usbsas_config::{conf_parse, conf_read};

let conf_str = conf_read("/etc/usbsas/config.toml")?;
// enter seccomp here: no more file opening
let config = conf_parse(&conf_str)?;
```

Reading and parsing are separate so that a process can read the file, then
sandbox itself, and only parse the (untrusted) content once sandboxed.

## Who uses what

Every process reads the same file and only uses the sections it needs:

| Section / option                 | Used by                                           |
|----------------------------------|---------------------------------------------------|
| `out_directory`, `keep_tmp_files`| usbsas, client                                    |
| `networks`                       | usbsas (destinations), uploader                   |
| `source_network`                 | usbsas (source), downloader                       |
| `src_local_dirs`                 | usbsas (sources), local2files                     |
| `command`                        | usbsas (destination), cmdexec                     |
| `post_copy`                      | usbsas, cmdexec                                   |
| `analyzer`                       | usbsas (whether to analyze), analyzer             |
| `filters`, `max_depth`           | usbsas                                            |
| `report.write_dest`              | usbsas (report written on the destination)        |
| `report.write_local`             | client (report saved locally)                     |
| `usb_port_accesses`              | usbdev (`ports_src`, `ports_dst`), hid-dealer (`ports_hid`) |
| `lang`, `menu_img`, `window_title` | client                                          |

`available_space` and `session_id` aren't read from the file: usbsas fills them
at runtime.
