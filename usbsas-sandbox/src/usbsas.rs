use crate::{seccomp, Result};
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub struct SocketFds {
    pub listen: RawFd,
    pub read: RawFd,
    pub write: RawFd,
}

pub fn seccomp(
    fds_read: Vec<RawFd>,
    fds_write: Vec<RawFd>,
    socket_fds: Option<SocketFds>,
) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(fds_read, fds_write)?;

    ctx.allow_syscall(Syscall::wait4)?;
    ctx.allow_syscall(Syscall::getrandom)?;
    ctx.allow_syscall(Syscall::uname)?;
    // XXX TODO Allow unlink but ensure with landlock only the socket file can be removed
    ctx.allow_syscall(Syscall::unlink)?;

    // Allow recvfrom and close on socket_fd
    if let Some(fds) = socket_fds {
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::close,
            &[Comparator::new(0, Cmp::Eq, fds.listen as u64, None)],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::recvfrom,
            &[Comparator::new(0, Cmp::Eq, fds.read as u64, None)],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::recvfrom,
            &[Comparator::new(0, Cmp::Eq, fds.write as u64, None)],
        )?;
    };

    ctx.load()?;

    Ok(())
}
