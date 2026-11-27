use crate::{Result, seccomp};
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

/// Main thread: communication with the parent only.
pub fn seccomp(fd_read: RawFd, fd_write: RawFd) -> Result<()> {
    let ctx = seccomp::new_context_with_common_rules(vec![fd_read], vec![fd_write])?;
    ctx.load()?;
    Ok(())
}

/// udev monitor thread, applied once the monitor listens and `poll_fd` is set up: receive events
/// from the udev monitor socket and wait on the poll instance.
pub fn seccomp_thread(udev_socket: RawFd, poll_fd: RawFd) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(vec![], vec![])?;

    // Receive events on udev's monitor socket
    for syscall in [Syscall::recvfrom, Syscall::recvmsg] {
        ctx.set_rule_for_syscall(
            Action::Allow,
            syscall,
            &[Comparator::new(
                0,
                Cmp::MaskedEq,
                u64::from(u32::MAX),
                Some(udev_socket as u64),
            )],
        )?;
    }

    // Wait on the polling fd
    ctx.set_rule_for_syscall(
        Action::Allow,
        #[cfg(not(target_arch = "aarch64"))]
        Syscall::epoll_wait,
        #[cfg(target_arch = "aarch64")]
        Syscall::epoll_pwait,
        &[Comparator::new(
            0,
            Cmp::MaskedEq,
            u64::from(u32::MAX),
            Some(poll_fd as u64),
        )],
    )?;

    // Close both if the thread returns (on error)
    for fd in [udev_socket, poll_fd] {
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::close,
            &[Comparator::new(
                0,
                Cmp::MaskedEq,
                u64::from(u32::MAX),
                Some(fd as u64),
            )],
        )?;
    }

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

    // Allocator
    ctx.allow_syscall(Syscall::madvise)?;
    ctx.allow_syscall(Syscall::getrandom)?;

    ctx.load()?;
    Ok(())
}

/// Scan thread, started once the monitor listens: list the devices already
/// plugged (reading sysfs and the udev database, restricted with landlock),
/// send them to the main thread and exit.
pub fn seccomp_scan() -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(vec![], vec![])?;

    // libudev (sd-device) may wait with ppoll()/poll() while enumerating
    // devices, e.g. at boot before udev has processed them. Without these the
    // whole process is intermittently killed (SIGSYS, syscall 271 on x86_64).
    // They only wait on already opened descriptors.
    ctx.allow_syscall(Syscall::ppoll)?;
    #[cfg(not(target_arch = "aarch64"))]
    ctx.allow_syscall(Syscall::poll)?;

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

    // Read sysfs and the udev database
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
    ctx.allow_syscall(Syscall::clock_nanosleep)?;

    // Allocator
    ctx.allow_syscall(Syscall::madvise)?;
    ctx.allow_syscall(Syscall::getrandom)?;

    ctx.load()?;
    Ok(())
}
