# usbsas-analyzer-server

Basic implementation of the remote server usbsas talks to: analysis with
[ClamAV](https://www.clamav.net/), upload and download of archives.

It's used by the integration tests and as an example of the API expected by
`usbsas-net`. **Production use is not recommended.**

## Usage

```shell
$ usbsas-analyzer-server [-d WORKING_DIR]
```

It listens on `127.0.0.1:8042` and starts `clamd` (which must be installed)
with a configuration in the working directory (a temporary directory in `/tmp`
by default).

## API

| Request                                        | Effect                                           |
|------------------------------------------------|--------------------------------------------------|
| `POST /api/scanbundle/<user id>`               | receive a tar and start analyzing its `data/` directory, answers `{"id": ..., "status": "uploaded"}` |
| `GET /api/scanbundle/<user id>/<analysis id>`  | status of the analysis (`processing`, `scanned` or `error`). Once done, the report: status (`CLEAN` / `DIRTY`) of each file and ClamAV versions |
| `POST /api/uploadbundle/<user id>`             | receive a tar (stored in the working directory)  |
| `HEAD /api/downloadbundle/<user id>/<pin>`     | uncompressed size of the archive in `X-Uncompressed-Content-Length` |
| `GET /api/downloadbundle/<user id>/<pin>`      | the archive `<working dir>/<user id>/<pin>.{tar,tar.gz,gz}` |
| `GET /shutdown`                                | stop clamd                                       |

The URLs to put in the usbsas configuration are then
`http://127.0.0.1:8042/api/scanbundle` (`[analyzer]`),
`http://127.0.0.1:8042/api/uploadbundle` (`[[networks]]`) and
`http://127.0.0.1:8042/api/downloadbundle` (`[source_network]`).

## Features

- `integration-tests`: deterministic ids and versions in responses, and a
  minimal ClamAV database only detecting the EICAR test file (faster startup).
