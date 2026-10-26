# usbsas-cmdexec

usbsas process executing the commands of the configuration on the output of a
transfer:

- the **command destination** (`[command]`): executed on the tar instead of
  writing files to a device or uploading them,
- the **post copy command** (`[post_copy]`): executed after any transfer, on
  the tar or the file system image.

`%SOURCE_FILE%` in the command arguments is replaced by the path of the output
file. A command failing (non zero exit status) is reported as an error, its
stdout and stderr are logged.

**This process is not sandboxed**, since it executes arbitrary commands.

## Usage

```
usbsas-cmdexec <TAR_PATH> <FS_PATH> [-c CONFIG_PATH]
```

It is spawned locked by `usbsas-usbsas` and waits for its unlock value (see
`usbsas-process`):

| Value | Meaning                                                        |
|-------|----------------------------------------------------------------|
| `0`   | not needed for this session, wait for `End`                    |
| `1`   | use `TAR_PATH`                                                 |
| `2`   | files were analyzed, use the clean tar (`<TAR_PATH>_clean.tar`) |

## Protocol

`cmdexec.proto3`, requester: `usbsas-usbsas`.

| Request        | Effect                                                        |
|----------------|---------------------------------------------------------------|
| `Exec`         | execute `[command]` on the tar                                |
| `PostCopyExec` | execute `[post_copy]` on the tar or the file system image (`outfiletype`) |
| `End`          | exit                                                          |

Each command can be executed only once per session.
