# usbsas-identifier

usbsas process that identifies the user of a transfer.

usbsas requires a user id before starting a transfer: it names the archive
downloaded from a network source and is sent to the analyzer and the uploader.

**This implementation is a placeholder**: it always answers the same id
(`Tartempion`). A real deployment is expected to replace it with an identifier
reading a badge, a smart card, etc. An empty id means the user isn't identified
and usbsas refuses to start a transfer.

## Protocol

`identifier.proto3`, requester: `usbsas-usbsas`.

| Request  | Response                                  |
|----------|-------------------------------------------|
| `UserId` | `ResponseUserId { userid }`               |
| `End`    | `End`, then exits                          |

## Sandbox

seccomp only, applied at startup before any request is parsed.
