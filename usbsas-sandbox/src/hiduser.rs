use crate::{seccomp, Result};
use syscallz::{Action, Cmp, Comparator, Syscall};

use std::os::unix::io::RawFd;

pub fn seccomp(device_fd: RawFd, x11_socket: RawFd) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(vec![], vec![])?;

    seccomp::apply_libusb_rules(&mut ctx, device_fd)?;

    // Allow recvmsg and writev on X11's socket
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::recvmsg,
        &[Comparator::new(0, Cmp::Eq, x11_socket as u64, None)],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::writev,
        &[Comparator::new(0, Cmp::Eq, x11_socket as u64, None)],
    )?;

    ctx.load()?;

    Ok(())
}
