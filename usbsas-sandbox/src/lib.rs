//! Sandboxing helpers for usbsas processes.

pub mod dev2scsi;
pub mod files2fs;
pub mod files2tar;
pub mod filter;
pub mod fs2dev;
pub mod fswriter;
pub mod identificator;
pub mod imager;
pub mod scsi2files;
pub mod tar2files;
pub mod usbdev;
pub mod usbsas;

pub(crate) mod seccomp;

use procfs::process::{FDTarget, Process};
use std::{os::unix::io::RawFd, path::PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("syscallz error: {0}")]
    Syscallz(#[from] syscallz::Error),
    #[error("procfs error: {0}")]
    Procfs(#[from] procfs::ProcError),
    #[error("{0}")]
    Error(String),
}
type Result<T> = std::result::Result<T, Error>;

pub struct LibusbFds {
    pub device: Option<RawFd>,
    pub timers: Vec<RawFd>,
    pub events: Vec<RawFd>,
}

// XXX Get those fds from rusb when possible
#[cfg(not(feature = "mock"))]
pub fn get_libusb_opened_fds(busnum: u32, devnum: u32) -> Result<LibusbFds> {
    let mut dev_fd = None;
    let mut event_fds = vec![];
    let mut timer_fds = vec![];

    for fd in Process::myself()?.fd()? {
        let fd = fd?;
        match fd.target {
            FDTarget::Path(path) => {
                if PathBuf::from(format!("/dev/bus/usb/{:03}/{:03}", busnum, devnum)) == path {
                    dev_fd = Some(fd.fd as RawFd);
                }
            }
            FDTarget::AnonInode(inode_type) => match inode_type.as_str() {
                "[timerfd]" => timer_fds.push(fd.fd as RawFd),
                "[eventfd]" => event_fds.push(fd.fd as RawFd),
                _ => (),
            },
            _ => (),
        }
    }
    Ok(LibusbFds {
        device: dev_fd,
        timers: timer_fds,
        events: event_fds,
    })
}

/* XXX: Functions returning constants we need and can't (yet) get from bindgen
 * because it cannot develop macro.
 * see: https://github.com/rust-lang/rust-bindgen/issues/753
 */
extern "C" {
    pub fn usbdevfs_submiturb() -> u64;
    pub fn usbdevfs_reapurbndelay() -> u64;
    pub fn usbdevfs_releaseinterface() -> u64;
    pub fn usbdevfs_ioctl() -> u64;
    pub fn usbdevfs_discardurb() -> u64;
    pub fn usbdevfs_get_capabilities() -> u64;
    pub fn usbdevfs_disconnect_claim() -> u64;
    pub fn usbdevfs_reset() -> u64;
}
