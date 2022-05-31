use crate::Result;
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn drop_priv(fd_read: RawFd, fd_write: RawFd, libusb_fds: crate::LibusbFds) -> Result<()> {
    let mut ctx = crate::new_context_with_common_rules(vec![fd_read], vec![fd_write])?;
    crate::apply_libusb_rules(&mut ctx, libusb_fds)?;
    ctx.load()?;

    Ok(())
}

pub fn thread_drop_priv(libusb_fds: crate::LibusbFds) -> Result<()> {
    let mut ctx = crate::new_context_with_common_rules(vec![], vec![])?;

    ctx.allow_syscall(Syscall::openat)?;
    ctx.allow_syscall(Syscall::read)?;
    ctx.allow_syscall(Syscall::write)?;
    ctx.allow_syscall(Syscall::close)?;
    ctx.allow_syscall(Syscall::recvmsg)?;

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

    crate::apply_libusb_rules(&mut ctx, libusb_fds)?;

    ctx.load()?;
    Ok(())
}
