//! Seccomp rules for usbsas processes.

pub mod dev2scsi;
pub mod files2fs;
pub mod files2tar;
pub mod filter;
pub mod fs2dev;
pub mod identificator;
pub mod imager;
pub mod scsi2files;
pub mod tar2files;
pub mod usbdev;
pub mod usbsas;

use procfs::process::{FDTarget, Process};
use std::{os::unix::io::RawFd, path::PathBuf};
use syscallz::{Action, Cmp, Comparator, Context, Syscall};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    Syscallz(#[from] syscallz::Error),
    #[error("io error: {0}")]
    Procfs(#[from] procfs::ProcError),
    #[error("{0}")]
    Error(String),
}
type Result<T> = std::result::Result<T, Error>;

pub(crate) fn new_context_with_common_rules(
    fds_read: Vec<RawFd>,
    fds_write: Vec<RawFd>,
) -> Result<Context> {
    let mut ctx = Context::init_with_action(Action::KillProcess)?;

    // Allow read
    for fd in &fds_read {
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::read,
            &[Comparator::new(0, Cmp::Eq, *fd as u64, None)],
        )?;
    }

    // Allow write
    for fd in &fds_write {
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::write,
            &[Comparator::new(0, Cmp::Eq, *fd as u64, None)],
        )?;
    }

    // Allow close
    for fd in fds_read.iter().chain(fds_write.iter()) {
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::close,
            &[Comparator::new(0, Cmp::Eq, *fd as u64, None)],
        )?;
    }

    // Allow write to stdout
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::write,
        &[Comparator::new(0, Cmp::Eq, 1, None)],
    )?;

    // Allow write to stderr
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::write,
        &[Comparator::new(0, Cmp::Eq, 2, None)],
    )?;

    // Allow mmap (for NULL addr only)
    ctx.set_rule_for_syscall(
        Action::Allow,
        #[cfg(not(target_arch = "arm"))]
        Syscall::mmap,
        #[cfg(target_arch = "arm")]
        Syscall::mmap2,
        &[Comparator::new(0, Cmp::Eq, 0, None)],
    )?;
    // Disallow mmap with PROT_EXEC
    ctx.set_rule_for_syscall(
        Action::KillThread,
        #[cfg(not(target_arch = "arm"))]
        Syscall::mmap,
        #[cfg(target_arch = "arm")]
        Syscall::mmap2,
        &[Comparator::new(
            2,
            Cmp::MaskedEq,
            libc::PROT_EXEC as u64,
            Some(libc::PROT_EXEC as u64),
        )],
    )?;

    // Allow mremap
    ctx.allow_syscall(Syscall::mremap)?;
    // but disallow with PROT_EXEC
    ctx.set_rule_for_syscall(
        Action::KillThread,
        Syscall::mremap,
        &[Comparator::new(
            2,
            Cmp::MaskedEq,
            libc::PROT_EXEC as u64,
            Some(libc::PROT_EXEC as u64),
        )],
    )?;

    // Allow more syscalls
    ctx.allow_syscall(Syscall::sigaltstack)?;
    ctx.allow_syscall(Syscall::munmap)?;
    ctx.allow_syscall(Syscall::exit_group)?;
    ctx.allow_syscall(Syscall::futex)?;
    ctx.allow_syscall(Syscall::brk)?;
    ctx.allow_syscall(Syscall::clock_gettime)?;
    #[cfg(target_arch = "arm")]
    ctx.allow_syscall(Syscall::clock_gettime64)?;
    ctx.allow_syscall(Syscall::rt_sigreturn)?;

    Ok(ctx)
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

pub(crate) fn apply_libusb_rules(ctx: &mut Context, libusb_fds: LibusbFds) -> Result<()> {
    if let Some(device_fd) = libusb_fds.device {
        // Allow close on device fd
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::close,
            &[Comparator::new(0, Cmp::Eq, device_fd as u64, None)],
        )?;

        // Allow some ioctls on device fd
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::ioctl,
            &[
                Comparator::new(0, Cmp::Eq, device_fd as u64, None),
                Comparator::new(1, Cmp::Eq, unsafe { usbdevfs_submiturb() }, None),
            ],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::ioctl,
            &[
                Comparator::new(0, Cmp::Eq, device_fd as u64, None),
                Comparator::new(1, Cmp::Eq, unsafe { usbdevfs_reapurbndelay() }, None),
            ],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::ioctl,
            &[
                Comparator::new(0, Cmp::Eq, device_fd as u64, None),
                Comparator::new(1, Cmp::Eq, unsafe { usbdevfs_releaseinterface() }, None),
            ],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::ioctl,
            &[
                Comparator::new(0, Cmp::Eq, device_fd as u64, None),
                Comparator::new(1, Cmp::Eq, unsafe { usbdevfs_ioctl() }, None),
            ],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::ioctl,
            &[
                Comparator::new(0, Cmp::Eq, device_fd as u64, None),
                Comparator::new(1, Cmp::Eq, unsafe { usbdevfs_discardurb() }, None),
            ],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::ioctl,
            &[
                Comparator::new(0, Cmp::Eq, device_fd as u64, None),
                Comparator::new(1, Cmp::Eq, unsafe { usbdevfs_get_capabilities() }, None),
            ],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::ioctl,
            &[
                Comparator::new(0, Cmp::Eq, device_fd as u64, None),
                Comparator::new(1, Cmp::Eq, unsafe { usbdevfs_disconnect_claim() }, None),
            ],
        )?;
    }

    // XXX poll() takes as first arg an array of struct pollfd, can we use comparators for this ?
    #[cfg(not(target_arch = "aarch64"))]
    ctx.allow_syscall(Syscall::poll)?;
    #[cfg(target_arch = "aarch64")]
    ctx.allow_syscall(Syscall::ppoll)?;

    // Allow read, write & close on eventfds
    for eventfd in libusb_fds.events {
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::read,
            &[Comparator::new(0, Cmp::Eq, eventfd as u64, None)],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::write,
            &[Comparator::new(0, Cmp::Eq, eventfd as u64, None)],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::close,
            &[Comparator::new(0, Cmp::Eq, eventfd as u64, None)],
        )?;
    }

    // Allow timerfd_settime and close on timerfds
    for timerfd in libusb_fds.timers {
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::timerfd_settime,
            &[Comparator::new(0, Cmp::Eq, timerfd as u64, None)],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::close,
            &[Comparator::new(0, Cmp::Eq, timerfd as u64, None)],
        )?;
    }

    Ok(())
}
