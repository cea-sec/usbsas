# usbsas-proto

Protobuf messages exchanged between usbsas processes.

Messages are defined in `proto/*.proto3` and compiled to Rust by
[prost](https://github.com/tokio-rs/prost) in `build.rs`. Comments in the proto
files become the documentation of the generated types (`cargo doc -p
usbsas-proto`). The Python module compiles the same files with `protoc` (see
[python/README.md](../python/README.md)).

The messages are framed and sent by `usbsas-comm`.

## Protocols

Each file is a protocol between a process and its parent:

| File               | Responder                                | Requester         |
|--------------------|------------------------------------------|-------------------|
| `usbsas.proto3`    | usbsas server                            | GUI, Python module, integration tests |
| `usbdev.proto3`    | usbdev                                   | usbsas            |
| `identifier.proto3`| identifier                               | usbsas            |
| `files.proto3`     | scsi2files, local2files, tar2files       | usbsas            |
| `scsi.proto3`      | dev2scsi                                 | scsi2files        |
| `writedst.proto3`  | files2tar, files2fs                      | usbsas            |
| `fs2dev.proto3`    | fs2dev                                   | usbsas            |
| `downloader.proto3`| downloader                               | usbsas            |
| `uploader.proto3`  | uploader                                 | usbsas            |
| `analyzer.proto3`  | analyzer                                 | usbsas            |
| `jsonparser.proto3`| jsonparser                               | analyzer          |
| `cmdexec.proto3`   | cmdexec                                  | usbsas            |
| `common.proto3`    | types shared by all protocols (devices, files, status, reports) | |

The tools of `usbsas-tools` (imager, fuse-mount...) also spawn some of these
processes directly.

Every protocol follows the same pattern:

- `RequestX` / `ResponseX` messages for each operation `X`,
- a `Request` message with a `oneof msg` listing the requests, always including
  `End`,
- a `Response` message with a `oneof msg` listing the responses, always
  including `End` (`common.ResponseEnd`), `Error` (`common.ResponseError`) and
  `Status` (`common.ResponseStatus`, progress of long operations).

## Rust helpers

On top of the generated code, `src/lib.rs` provides in the `common` module:

- `device::Device` accessors (`is_src`, `is_dst`, `title`, `description`) and
  `id()`, the identifier used to select devices in requests,
- conversions from `usbsas-config` types (`Network`, `LocalDir`, `Command`),
- conversion of a device to its report form (`DeviceReport`), which leaves out
  internal details,
- `Display` for `UsbDevice` and `FsType`.

`build.rs` also derives `serde` traits on report types (serialized to JSON) and
on `AnalyzeReport` / `SrvResp` (parsed from the analyzer server's JSON), and
uses `BTreeMap` for maps so that reports are ordered and hashable.

## Adding a message

1. Add the `RequestX` / `ResponseX` messages and their entries in the `Request`
   / `Response` oneofs, with a comment describing them.
2. Add `X` to the `protorequest!` / `protoresponse!` invocations in
   `usbsas-comm`.
3. If it's part of the `usbsas` protocol, update `python/comm.py`.
