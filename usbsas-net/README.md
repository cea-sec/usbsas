# usbsas-net

usbsas processes talking to remote HTTP servers:

| Binary              | Role                                                              |
|---------------------|-------------------------------------------------------------------|
| `usbsas-analyzer`   | sends the session tar to the analyzer (antivirus) and gets its report |
| `usbsas-jsonparser` | parses the analyzer's JSON responses, spawned by the analyzer     |
| `usbsas-uploader`   | uploads the session tar (or clean tar) to a destination network   |
| `usbsas-downloader` | downloads an archive from the source network into the session tar |

The remote API is described in the [usage documentation](../doc/usage.md#asrv)
and implemented by `usbsas-analyzer-server`, used for tests.

## Usage

All but the jsonparser take the session tar and the configuration:

```
usbsas-analyzer <TAR_PATH> [-c CONFIG_PATH]
usbsas-uploader <TAR_PATH> [-c CONFIG_PATH]
usbsas-downloader <TAR_PATH> [-c CONFIG_PATH]
usbsas-jsonparser
```

They are spawned by `usbsas-usbsas`. A process with no matching section in the
configuration (`[analyzer]`, `[[networks]]`, `[source_network]`) only waits for
`End`.

## Remote API

- **analyzer**: `POST <url>/<user id>` with the tar, the answer gives an
  analysis id. Then `GET <url>/<user id>/<analysis id>` every second until the
  status is `scanned`; the response is the `AnalyzeReport` (status of each
  file).
- **uploader**: `POST <url>/<user id>` with the tar. It's spawned locked and
  waits for its unlock value: `1` to upload the tar, `2` the clean tar (files
  were analyzed), `0` if not needed.
- **downloader**: `HEAD <url>/<user id>/<pin>` must answer the uncompressed
  size of the archive in the `X-Uncompressed-Content-Length` header, then `GET`
  downloads it (gzip encoding is supported).

Progress is reported to usbsas with `Status` responses (`UploadAv`,
`UploadDst`, `DlSrc`).

## Sandbox

seccomp and landlock (`usbsas_sandbox::net`): read only access to system files
needed by TLS and DNS resolution (`/etc/ssl`, `/etc/resolv.conf`, libraries...),
access to the tar only, and TCP connections only to the ports of the configured
URLs (and 88 for Kerberos).

The JSON responses of the server are parsed in a separate process
(`usbsas-jsonparser`) with a strict seccomp filter: it can only talk to the
analyzer.

## Features

- `authkrb` (default): Kerberos authentication (SPNEGO `Negotiate`, with
  `libgssapi`) when a `krb_service_name` is configured. A ticket must be
  acquired by other means.
- `native-tls`: use the system TLS library instead of rustls.
