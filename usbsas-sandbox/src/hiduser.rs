use crate::{Result, seccomp};
use syscallz::{Action, Cmp, Comparator, Syscall};

use std::os::unix::io::RawFd;

pub fn seccomp(device_fd: RawFd, uinput_device: RawFd) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(vec![], vec![])?;

    seccomp::apply_libusb_rules(&mut ctx, device_fd)?;

    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::write,
        &[Comparator::new(0, Cmp::Eq, uinput_device as u64, None)],
    )?;

    ctx.allow_syscall(Syscall::poll)?;

    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, 1, None),
            #[cfg(target_env = "musl")]
            Comparator::new(1, Cmp::Eq, libc::TCGETS as u64, None),
            #[cfg(not(target_env = "musl"))]
            Comparator::new(1, Cmp::Eq, libc::TCGETS, None),
        ],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, 1, None),
            #[cfg(target_env = "musl")]
            Comparator::new(1, Cmp::Eq, libc::TCGETS2 as u64, None),
            #[cfg(not(target_env = "musl"))]
            Comparator::new(1, Cmp::Eq, libc::TCGETS2, None),
        ],
    )?;

    ctx.load()?;

    Ok(())
}
