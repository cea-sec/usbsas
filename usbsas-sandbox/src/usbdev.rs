use crate::{seccomp, Result};
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn seccomp(fd_read: RawFd, fd_write: RawFd) -> Result<()> {
    let ctx = seccomp::new_context_with_common_rules(vec![fd_read], vec![fd_write])?;
    ctx.load()?;
    Ok(())
}

pub fn seccomp_thread(udev_socket: RawFd, poll_fd: RawFd) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(vec![], vec![])?;

    // Allow some syscalls on udev's monitor socket
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::setsockopt,
        &[Comparator::new(0, Cmp::Eq, udev_socket as u64, None)],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::bind,
        &[Comparator::new(0, Cmp::Eq, udev_socket as u64, None)],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::getsockname,
        &[Comparator::new(0, Cmp::Eq, udev_socket as u64, None)],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::recvfrom,
        &[Comparator::new(0, Cmp::Eq, udev_socket as u64, None)],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::recvmsg,
        &[Comparator::new(0, Cmp::Eq, udev_socket as u64, None)],
    )?;

    // Allow some syscalls on the polling fd
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::epoll_ctl,
        &[Comparator::new(0, Cmp::Eq, poll_fd as u64, None)],
    )?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        #[cfg(not(target_arch = "aarch64"))]
        Syscall::epoll_wait,
        #[cfg(target_arch = "aarch64")]
        Syscall::epoll_pwait,
        &[Comparator::new(0, Cmp::Eq, poll_fd as u64, None)],
    )?;

    // Allow mprotect without PROT_EXEC
    ctx.allow_syscall(Syscall::mprotect)?;
    ctx.set_rule_for_syscall(
        Action::KillThread,
        Syscall::mprotect,
        &[Comparator::new(
            2,
            Cmp::MaskedEq,
            libc::PROT_EXEC as u64,
            Some(libc::PROT_EXEC as u64),
        )],
    )?;

    ctx.allow_syscall(Syscall::fstat)?;
    #[cfg(not(target_arch = "aarch64"))]
    ctx.allow_syscall(Syscall::lstat)?;
    ctx.allow_syscall(Syscall::fstatfs)?;
    ctx.allow_syscall(Syscall::newfstatat)?;
    ctx.allow_syscall(Syscall::statx)?;
    #[cfg(not(target_arch = "aarch64"))]
    ctx.allow_syscall(Syscall::access)?;
    ctx.allow_syscall(Syscall::faccessat2)?;
    ctx.allow_syscall(Syscall::getdents64)?;
    ctx.allow_syscall(Syscall::readlinkat)?;
    ctx.allow_syscall(Syscall::openat)?;
    ctx.allow_syscall(Syscall::read)?;
    ctx.allow_syscall(Syscall::close)?;
    ctx.allow_syscall(Syscall::getrandom)?;
    ctx.allow_syscall(Syscall::clock_nanosleep)?;

    ctx.load()?;
    Ok(())
}
