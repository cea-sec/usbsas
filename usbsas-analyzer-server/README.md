# usbsas-analyzer-server

Remote server for usbsas that does three things:

- **analyzes** bundles with ClamAV (`/api/scanbundle`),
- **stores** bundles uploaded by usbsas (`/api/uploadbundle`),
- **serves** bundles for usbsas to download (`/api/downloadbundle`).

It's the reference implementation of the network side of usbsas, used by the
integration tests. It handles several users concurrently: analyses run in
parallel, bundles are stored per user, and a user can only read their own
reports and bundles.

## Running

```sh
usbsas-analyzer-server --working-dir /var/lib/usbsas-analyzer-server
```

The server starts its own `clamd` (with a configuration generated in
`WORKING-DIR/.clamd/`), waits for its database to load, then listens on
`127.0.0.1:8042`. If `clamd` dies, it is restarted automatically. It is also
stopped when the server exits, even if the server is killed with `SIGKILL`.

| Option                           | Default            | Description                                                                                  |
|----------------------------------|--------------------|----------------------------------------------------------------------------------------------|
| `-d, --working-dir DIR`          | temporary dir      | Where bundles are stored. A temporary directory is removed at exit, along with every bundle in it. |
| `-b, --bind ADDR:PORT`           | `127.0.0.1:8042`   | Listening address.                                                                           |
| `--workers N`                    | number of CPUs     | HTTP worker threads.                                                                         |
| `--max-concurrent-scans N`       | number of CPUs     | Analyses running at the same time; the others wait (status `processing`).                    |
| `--max-bundle-size BYTES`        | unlimited          | Larger uploads are rejected with 413.                                                        |
| `--result-ttl SECONDS`           | `3600`             | Analysis reports never fetched are dropped after this delay.                                 |
| `--clamd PATH`                   | `clamd`            | clamd binary (looked up in `/bin:/sbin:/usr/bin:/usr/sbin:/usr/local/bin:/usr/local/sbin`).   |
| `--clamd-config FILE`            | none               | Extra clamd directives appended to the generated configuration.                              |
| `--clamd-start-timeout SECONDS`  | `300`              | How long to wait for clamd to load its database.                                             |
| `--scan-timeout SECONDS`         | `0` (no limit)     | Maximum duration of the scan of one bundle.                                                  |
| `--disable-shutdown`             | off                | Disable `GET /shutdown` (do this in production).                                             |

Logging uses `RUST_LOG` (default `info`).

### clamd limits

With its default configuration, clamd only scans the first 100 MB of a file
(`MaxFileSize`, `MaxScanSize`, ...) and reports the file as clean. To treat
these files as infected instead, use `--clamd-config` with a file containing:

```
AlertExceedsMax yes
```

Other useful directives: `MaxThreads`, `MaxFileSize`, `MaxScanSize`,
`DatabaseDirectory`.

## API

`USER` is the user id sent by usbsas, `BUNDLE` is a bundle id. Ids must match
`[A-Za-z0-9._@+-]{1,128}` and can't start with `.`; otherwise the server
answers `400`.

### `POST /api/scanbundle/USER`

The body is the tar to analyze. Once it's fully received, the server answers:

```json
{ "id": "BUNDLE", "status": "uploaded" }
```

The analysis then runs in the background.

### `GET /api/scanbundle/USER/BUNDLE`

While the analysis is queued or running:

```json
{ "id": "BUNDLE", "status": "processing" }
```

When it's done, the server returns the report and then forgets it, so a
second request gets `404`:

```json
{
  "id": "BUNDLE",
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

- Only regular files and symlinks under the tar's `data/` directory are
  reported, with paths relative to `data/`.
- A file is `DIRTY` if clamd found something in it **or couldn't scan it**.
- `status` is `error` (with an empty `files`) if the tar can't be read,
  contains no file under `data/`, or clamd fails.
- The server answers `404` if the analysis is unknown, was already fetched,
  has expired, or was submitted by another user.

### `POST /api/uploadbundle/USER`

The body is a tar. The server stores it as `WORKING-DIR/USER/BUNDLE.tar` and
answers `{"id": "BUNDLE", "status": "uploaded"}`. The bundle can then be
downloaded with `/api/downloadbundle/USER/BUNDLE`.

### `HEAD /api/downloadbundle/USER/BUNDLE`

Returns the `X-Uncompressed-Content-Length` header: the size of the tar, after
decompression if the bundle is gzipped. The server looks for
`WORKING-DIR/USER/BUNDLE.tar`, then `.tar.gz`, then `.tgz`.

For gzipped bundles, the server decompresses the whole file to compute the
size. It can't read the size from the gzip trailer, because the trailer
stores it modulo 4 GiB. The result is cached until the file changes. This
also checks the file's integrity: a corrupted file gives a `500` instead of a
wrong size.

### `GET /api/downloadbundle/USER/BUNDLE`

Returns the bundle. Gzipped bundles are sent as they are, with
`Content-Encoding: gzip`. Range requests are supported.

### `GET /shutdown`

Stops clamd and then the server, gracefully. The integration tests use it.
`--disable-shutdown` turns it off.

## Working directory layout

```
WORKING-DIR/
├── USER/               one directory per user
│   ├── BUNDLE.tar      uploaded bundles, or bundles to download put here
│   ├── BUNDLE.tar.gz   by an operator or another service
│   └── BUNDLE.tgz
├── .scans/             bundles being analyzed (cleared at startup)
├── .tmp/               bodies being received (cleared at startup)
└── .clamd/             clamd.conf, clamd.sock, clamd's temporary files
```

Uploads are written to `.tmp/`, synced, then renamed into place. A reader
never sees a partial bundle, and an interrupted upload leaves nothing behind.
The path of the clamd socket must stay under 108 bytes, so keep the working
directory path reasonably short.

## Analysis details

For each bundle:

1. The tar is stored in `.scans/BUNDLE.tar`.
2. It waits for a free slot (`--max-concurrent-scans`).
3. Its regular files are extracted into a private directory under generated
   names (their index in the archive). Original names are only kept in
   memory. So tar paths can't escape the directory (`..`, absolute paths,
   symlinks), odd names (newlines, `": "`, non-UTF-8...) can't break clamd's
   output, and permissions stored in the archive can't stop clamd from
   reading a file.
4. clamd scans the directory (`CONTSCAN`) and the results are mapped back
   to the original paths.
5. The tar and the extracted files are deleted, and the report is kept until
   it's fetched or expires.

## Feature `integration-tests`

- Bundle ids are always `bundle_test`.
- The report id is `0` and the antivirus information is fixed, so the report
  written on the destination device is deterministic.
- clamd uses a minimal database that only detects EICAR, which loads fast.
