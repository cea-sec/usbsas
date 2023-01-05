use crate::{seccomp, Result};
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn seccomp(fd_read: RawFd, fd_write: RawFd, libusb_fds: crate::LibusbFds) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(vec![fd_read], vec![fd_write])?;
    seccomp::apply_libusb_rules(&mut ctx, libusb_fds)?;
    ctx.load()?;

    Ok(())
}

pub fn thread_seccomp(libusb_fds: crate::LibusbFds) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(vec![], vec![])?;

    ctx.allow_syscall(Syscall::openat)?;
    ctx.allow_syscall(Syscall::read)?;
    ctx.allow_syscall(Syscall::write)?;
    ctx.allow_syscall(Syscall::close)?;
    ctx.allow_syscall(Syscall::recvmsg)?;
    ctx.allow_syscall(Syscall::recvfrom)?;
    ctx.allow_syscall(Syscall::clock_nanosleep)?;

    // Allow some ioctls
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[Comparator::new(
            1,
            Cmp::Eq,
            unsafe { crate::usbdevfs_submiturb() },
            None,
        )],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[Comparator::new(
            1,
            Cmp::Eq,
            unsafe { crate::usbdevfs_reapurbndelay() },
            None,
        )],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[Comparator::new(
            1,
            Cmp::Eq,
            unsafe { crate::usbdevfs_releaseinterface() },
            None,
        )],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[Comparator::new(
            1,
            Cmp::Eq,
            unsafe { crate::usbdevfs_ioctl() },
            None,
        )],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[Comparator::new(
            1,
            Cmp::Eq,
            unsafe { crate::usbdevfs_discardurb() },
            None,
        )],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[Comparator::new(
            1,
            Cmp::Eq,
            unsafe { crate::usbdevfs_get_capabilities() },
            None,
        )],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[Comparator::new(
            1,
            Cmp::Eq,
            unsafe { crate::usbdevfs_disconnect_claim() },
            None,
        )],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[Comparator::new(
            1,
            Cmp::Eq,
            unsafe { crate::usbdevfs_reset() },
            None,
        )],
    )?;

    seccomp::apply_libusb_rules(&mut ctx, libusb_fds)?;

    ctx.load()?;
    Ok(())
}
