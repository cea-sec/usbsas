//! Seccomp helpers for usbsas processes.

use crate::Result;
use procfs::process::{FDTarget, Process};
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Context, Syscall};

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

    // Allow ioctl(2, TCGETS, ..), needed by env_logger >= 0.11
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, 2, None),
            #[cfg(target_env = "musl")]
            Comparator::new(1, Cmp::Eq, libc::TCGETS as u64, None),
            #[cfg(not(target_env = "musl"))]
            Comparator::new(1, Cmp::Eq, libc::TCGETS, None),
        ],
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

    //  fcntl only needed in debug mode
    #[cfg(debug_assertions)]
    ctx.allow_syscall(Syscall::fcntl)?;

    Ok(ctx)
}

pub(crate) fn apply_libusb_rules(ctx: &mut Context, device_fd: RawFd) -> Result<()> {
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
            Comparator::new(1, Cmp::Eq, unsafe { crate::usbdevfs_submiturb() }, None),
        ],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, device_fd as u64, None),
            Comparator::new(1, Cmp::Eq, unsafe { crate::usbdevfs_reapurbndelay() }, None),
        ],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, device_fd as u64, None),
            Comparator::new(
                1,
                Cmp::Eq,
                unsafe { crate::usbdevfs_releaseinterface() },
                None,
            ),
        ],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, device_fd as u64, None),
            Comparator::new(1, Cmp::Eq, unsafe { crate::usbdevfs_ioctl() }, None),
        ],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, device_fd as u64, None),
            Comparator::new(1, Cmp::Eq, unsafe { crate::usbdevfs_discardurb() }, None),
        ],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, device_fd as u64, None),
            Comparator::new(
                1,
                Cmp::Eq,
                unsafe { crate::usbdevfs_get_capabilities() },
                None,
            ),
        ],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, device_fd as u64, None),
            Comparator::new(
                1,
                Cmp::Eq,
                unsafe { crate::usbdevfs_disconnect_claim() },
                None,
            ),
        ],
    )?;

    // XXX poll() takes as first arg an array of struct pollfd, can we use comparators for this ?
    #[cfg(not(target_arch = "aarch64"))]
    ctx.allow_syscall(Syscall::poll)?;
    #[cfg(target_arch = "aarch64")]
    ctx.allow_syscall(Syscall::ppoll)?;

    let mut event_fds = vec![];
    let mut timer_fds = vec![];

    for fd in Process::myself()?.fd()? {
        let fd = fd?;
        if let FDTarget::AnonInode(inode_type) = fd.target {
            match inode_type.as_str() {
                "[timerfd]" => timer_fds.push(fd.fd as RawFd),
                "[eventfd]" => event_fds.push(fd.fd as RawFd),
                _ => (),
            }
        };
    }

    // Allow read, write & close on eventfds
    for eventfd in event_fds {
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
    for timerfd in timer_fds {
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
