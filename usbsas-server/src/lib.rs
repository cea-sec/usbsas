//! usbsas WEB server. This WEB server manages the usbsas parent process
//! (starting and resetting) and exposes an API on which clients can perform
//! transfers.

pub mod appstate;
pub(crate) mod error;
pub mod server;
pub(crate) mod srv_infos;
