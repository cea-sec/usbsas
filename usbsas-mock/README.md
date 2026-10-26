# usbsas-mock

Fake USB devices, to test usbsas without real hardware.

When usbsas is built with the `mock` feature (`cargo build --all --features
mock`), the processes accessing USB devices use this crate instead:

- `usbdev` is replaced by `MockUsbDev`, which lists:
  - an input device (bus 1, device 1, source only) if `USBSAS_MOCK_IN_DEV` is
    set,
  - an output device (bus 1, device 2, destination only) if
    `USBSAS_MOCK_OUT_DEV` is set.
- `dev2scsi` and `fs2dev` use `MockMassStorage` instead of `MassStorage`: it
  reads / writes the file given by `USBSAS_MOCK_IN_DEV` / `USBSAS_MOCK_OUT_DEV`
  as if it were the device (512 bytes sectors). These two processes don't apply
  their seccomp filter in this mode.

This is what the integration tests of `usbsas-usbsas` and `start_usbsas()` of
the Python module use.
