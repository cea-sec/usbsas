# usbsas-client

Graphical user interface for usbsas, built with [iced](https://iced.rs).

It is designed for the [kiosk](../doc/kiosk.md) use case: a single full screen
window, usable with a touch screen or a mouse, that guides the user through:

- transferring files from a USB device (or a network / local source) to another
  USB device or a remote server
- wiping a USB device
- making an image of a USB device

The client does not touch USB devices itself. It is a thin front-end that talks
to the `usbsas` server (`usbsas-usbsas -s`) over a Unix domain socket, using the
protobuf messages defined in `usbsas-proto` and the async helpers of
`usbsas-comm`. All the device and file system work happens in the server's
sandboxed processes.

## Usage

```shell
$ ./target/release/usbsas-usbsas -s
$ ./target/release/usbsas-client
```

Options:

* `-c, --config`: usbsas configuration file (default `/etc/usbsas/config.toml`)
* `-s, --socket`: path of the usbsas server socket (default: `/run/usbsas/usbsas.sock`)
* `-f, --fullscreen`: keep the window in fullscreen mode
* `-W, --width`: window width (default `1000`)
* `-H, --height`:  window height (default `800`)
* `-n, --nosandbox`: don't sandbox the client (landlock + seccomp)

The client reads the same [configuration file](../doc/configuration.md) as the
server. It uses, among others, `lang`, `menu_img`, `out_directory`,
`keep_tmp_files` and `report.write_local` (transfer reports are written as JSON
files in this directory).

Log level is set with `RUST_LOG` (see `env_logger`).

## Packaging

`cargo deb -p usbsas-client` builds the `usbsas-kiosk` Debian package. It
creates a `usbsas-client` user and a systemd unit that runs the client in
fullscreen with the [cage](https://github.com/cage-kiosk/cage) Wayland
compositor on tty2. See [kiosk deployment](../doc/kiosk.md).

## Code overview

The application follows iced's Elm-like architecture:

- `lib.rs`: the `GUI` state, its `State` machine, the `Message` and `Response`
  types, command line arguments, sandboxing, connection and session reset.
- `update.rs`: `GUI::update`, handles user and internal messages. Requests to
  the server run asynchronously so the interface never blocks; their results
  come back as `Message::Response`.
- `view.rs`: `GUI::view`, renders the screen corresponding to the current
  `State`.
- `subscription.rs`: a one second tick used to poll devices and drive the
  sandbox / connection steps.
- `components.rs`: shared widget styles and sizes.
- `resources/`: translations (`i18n/*.json`) and images embedded in the binary.
