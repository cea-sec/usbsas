use crate::{seccomp, Result};
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub struct UsbsasSocket {
    pub listen: RawFd,
    pub read: RawFd,
    pub write: RawFd,
    pub path: String,
}

pub fn sandbox(
    fds_read: Vec<RawFd>,
    fds_write: Vec<RawFd>,
    socket: Option<UsbsasSocket>,
    paths_rm: Option<&[&str]>,
) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(fds_read, fds_write)?;

    ctx.allow_syscall(Syscall::wait4)?;
    ctx.allow_syscall(Syscall::getrandom)?;
    ctx.allow_syscall(Syscall::uname)?;

    // Allow recvfrom and close and unlink on listen fd socket
    if let Some(sock) = socket {
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::close,
            &[Comparator::new(0, Cmp::Eq, sock.listen as u64, None)],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::recvfrom,
            &[Comparator::new(0, Cmp::Eq, sock.read as u64, None)],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::sendto,
            &[Comparator::new(0, Cmp::Eq, sock.write as u64, None)],
        )?;
    };

    // Allow unlink syscall but restrict it to socket path with landlock
    #[cfg(not(target_arch = "aarch64"))]
    ctx.allow_syscall(Syscall::unlink)?;
    #[cfg(target_arch = "aarch64")]
    ctx.allow_syscall(Syscall::unlinkat)?;

    crate::landlock(None, None, None, paths_rm, None)?;

    ctx.load()?;

    Ok(())
}
