# usbsas-sandbox

Sandboxing of usbsas processes, with [seccomp] and [landlock].

Every usbsas process opens what it needs (pipes to its parent, the device, the
output file...) and then applies its own filter, **before** reading any request
or untrusted data. From then on, any syscall that isn't allowed kills the
process.

## seccomp

There is one module per process (`dev2scsi`, `scsi2files`, `files2fs`,
`usbsas`...). Each one starts from common rules:

- `read` / `write` / `close` only on the given file descriptors (pipes to the
  parent and children, opened files), plus stdout / stderr for logging,
- memory allocation, but never with `PROT_EXEC`,
- a few harmless syscalls (`exit`, `sigaltstack`, `futex`...).

It then adds what the process needs: `lseek` on its file, the `USBDEVFS_*`
ioctls and eventfds / timerfds used by libusb for the processes accessing USB
devices (`apply_libusb_rules`), sockets for the network processes, etc.

## landlock

`landlock()` restricts file system and TCP access for processes that can't be
confined by seccomp alone, e.g. because the files they open aren't known in
advance:

- usbdev: read only access to the configuration, `/sys` and `/run/udev`,
- local2files: read only access to the configured source directories,
- usbsas: only creating and removing files in the output directory and the
  socket directory,
- network processes: only connecting to the configured ports,
- the client: only the paths it needs (GPU, socket, reports...).

By default, landlock is applied in best effort mode: on a kernel without (full)
landlock support, a warning is logged and the process continues. With the
`landlock-enforce` feature, an unsupported rule is an error.

`build.rs` compiles a small C file to get the values of the `USBDEVFS_*` ioctl
numbers, which bindgen can't generate.

[seccomp]: https://man7.org/linux/man-pages/man2/seccomp.2.html
[landlock]: https://docs.kernel.org/userspace-api/landlock.html
