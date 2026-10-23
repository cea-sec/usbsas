# usbsas-comm

Typed communication channels between usbsas processes.

Every usbsas process talks to its parent with protobuf messages (defined in
`usbsas-proto/proto/*.proto3`) over a pair of pipes or a Unix socket. This
crate implements the wire format and generates, for each protocol, a typed API
for both sides: the requester (parent) and the responder (child).

## Wire format

Each message is a protobuf `Request` or `Response` prefixed with its encoded
length as a 64 bit little endian integer:

```
┌──────────────────┬──────────────────────────┐
│ length (u64, LE) │ protobuf message         │
└──────────────────┴──────────────────────────┘
```

Messages larger than `MAX_PROTO_MESS_SIZE` (32 MiB, from `usbsas-utils`) are
rejected on reception. The Python module (`python/comm.py`) uses the same
format.

Communication is synchronous request / response: the requester sends a
`Request`, the responder answers with exactly one `Response`, except for long
operations where the responder sends a stream of `Status` responses until one
has `done` set. Any request can be answered with an `Error` response, and every
protocol has an `End` request that makes the responder exit.

## Usage

For each protocol `X` (`Usbsas`, `Files`, `Fs2Dev`, `WriteDst`...), the crate
provides:

| Side      | Channel type | Trait          | Methods                                        |
|-----------|--------------|----------------|------------------------------------------------|
| requester | `ComRqX`     | `ProtoReqX`    | one per request, e.g. `readdir(RequestReadDir) -> ResponseReadDir`, plus `end()` and `recv_status()` (`ProtoReqCommon`) |
| responder | `ComRpX`     | `ProtoRespX`   | `recv_req()`, one per response, e.g. `readdir(ResponseReadDir)`, plus `status()`, `done()`, `error()` and `end()` (`ProtoRespCommon`) |

Requester side (parent):

```rust
use usbsas_comm::{ComRqUsbDev, ProtoReqUsbDev};

// usually obtained from usbsas_process::UsbsasChildSpawner::spawn()
let devices = usbdev.comm.devices(usbsas_proto::usbdev::RequestDevices {})?.devices;
```

A `Response::Error` from the responder is returned as an `io::Error`.

Responder side (child):

```rust
use usbsas_comm::{ComRpUsbDev, Comm, ProtoRespCommon, ProtoRespUsbDev};
use usbsas_proto::usbdev::{ResponseDevices, request::Msg};

// pipes inherited from the parent (INPUT_PIPE_FD / OUTPUT_PIPE_FD)
let mut comm: ComRpUsbDev = Comm::from_env()?;
loop {
    match comm.recv_req()? {
        Msg::Devices(_) => comm.devices(ResponseDevices { devices })?,
        Msg::End(_) => {
            comm.end()?;
            break;
        }
    }
}
```

`Comm` also implements `Read` and `Write` directly, which is used for the raw
unlock value of locked children (see `usbsas-process`).

## Features

- `async`: adds `CommAsync`, a runtime agnostic (`futures-io`) version of
  `Comm`, and `ComRqAsyncUsbsas`, an async requester for the `Usbsas`
  protocol. It is used by the GUI client so that it stays responsive while
  waiting for the server. A request future must be driven to completion: if
  it's dropped half-way, the stream is desynchronized.

## Adding a request

1. Add the `RequestX` / `ResponseX` messages and the `oneof` entries in the
   `.proto3` file.
2. Add `X` to the `protorequest!` and `protoresponse!` invocations of the
   protocol at the end of `src/lib.rs` (and to `protorequest_async!` in
   `src/comm_async.rs` for the `Usbsas` protocol).
