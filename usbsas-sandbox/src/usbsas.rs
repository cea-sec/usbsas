use crate::{Result, seccomp};
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

/// Unix socket of the server, when used.
pub struct UsbsasSocket {
    /// Listening socket
    pub listen: RawFd,
    /// Connected socket, read side
    pub read: RawFd,
    /// Connected socket, write side
    pub write: RawFd,
    /// Path of the socket
    pub path: String,
}

/// seccomp: communication with the children (`fds_read` / `fds_write`) and the
/// client, waiting children, and removing / renaming files restricted with
/// landlock to `paths_rm` (output directory and socket).
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
            &[Comparator::new(
                0,
                Cmp::MaskedEq,
                u64::from(u32::MAX),
                Some(sock.listen as u64),
            )],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::recvfrom,
            &[Comparator::new(
                0,
                Cmp::MaskedEq,
                u64::from(u32::MAX),
                Some(sock.read as u64),
            )],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::sendto,
            &[Comparator::new(
                0,
                Cmp::MaskedEq,
                u64::from(u32::MAX),
                Some(sock.write as u64),
            )],
        )?;
    };

    // Allow unlink & rename but restrict it to out dir & socket path with landlock
    #[cfg(not(target_arch = "aarch64"))]
    ctx.allow_syscall(Syscall::unlink)?;
    #[cfg(target_arch = "aarch64")]
    ctx.allow_syscall(Syscall::unlinkat)?;
    ctx.allow_syscall(Syscall::rename)?;

    crate::landlock(None, None, None, paths_rm, None)?;

    ctx.load()?;

    Ok(())
}
