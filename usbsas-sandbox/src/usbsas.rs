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

/// seccomp: communication with the socket and the epoll fd,
/// kill on children pids with SIG(TERM|INT)
pub fn sandbox_epoll_thread(
    killable_pids: &[u32],
    socket_fd: RawFd,
) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(vec![], vec![])?;

    // epoll-related syscalls
    ctx.allow_syscall(Syscall::epoll_create1)?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::epoll_ctl,
        &[
            Comparator::new(
                1,
                Cmp::MaskedEq,
                u64::from(u32::MAX),
                Some(libc::EPOLL_CTL_ADD as u64),
            ),
            Comparator::new(
                2,
                Cmp::MaskedEq,
                u64::from(u32::MAX),
                Some(socket_fd as u64),
            )
        ],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        #[cfg(not(target_arch = "aarch64"))]
        Syscall::epoll_wait,
        #[cfg(target_arch = "aarch64")]
        Syscall::epoll_pwait,
        &[
            Comparator::new(
                2,
                Cmp::MaskedEq,
                u64::from(u32::MAX),
                Some(1),
            ),
            Comparator::new(
                3,
                Cmp::MaskedEq,
                u64::from(u32::MAX),
                Some(-1i32 as u64),
            )
        ],
    )?;

    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::shutdown,
        &[
            Comparator::new(
                0,
                Cmp::MaskedEq,
                u64::from(u32::MAX),
                Some(socket_fd as u64),
            ),
            Comparator::new(
                1,
                Cmp::MaskedEq,
                u64::from(u32::MAX),
                Some(libc::SHUT_RDWR as u64),
            )
        ],
    )?;

    // it's impractical to allow close on the epoll fd so we allow close on all fds
    ctx.allow_syscall(Syscall::close)?;

    // allow killable pids to be SIGTERMed or SIGKILLed
    for pid in killable_pids {
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::kill,
            &[
                Comparator::new(
                    0,
                    Cmp::MaskedEq,
                    u64::from(u32::MAX),
                    Some(*pid as u64)
                ),
                Comparator::new(
                    1,
                    Cmp::MaskedEq,
                    u64::from(u32::MAX),
                    Some(libc::SIGTERM as u64)
                ),
            ],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::kill,
            &[
                Comparator::new(
                    0,
                    Cmp::MaskedEq,
                    u64::from(u32::MAX),
                    Some(*pid as u64)
                ),
                Comparator::new(
                    1,
                    Cmp::MaskedEq,
                    u64::from(u32::MAX),
                    Some(libc::SIGKILL as u64)
                ),
            ],
        )?;
    }

    ctx.allow_syscall(Syscall::clock_nanosleep)?;

    // probably stems from rust optimizations
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::madvise,
        &[
            Comparator::new(
                2,
                Cmp::MaskedEq,
                u64::from(u32::MAX),
                Some(libc::MADV_DONTNEED as u64)
            ),
        ],
    )?;

    crate::landlock(None, None, None, None, None)?;

    ctx.load()?;

    Ok(())
}
