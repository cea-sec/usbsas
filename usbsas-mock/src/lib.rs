//! Mock usbsas mass_storage struct and usbdev process to test usbsas without
//! USB devices.
//!
//! Used when usbsas is built with the `mock` feature. Fake devices are files
//! given by the `USBSAS_MOCK_IN_DEV` and `USBSAS_MOCK_OUT_DEV` environment
//! variables.

pub mod mass_storage;
pub mod usbdev;
