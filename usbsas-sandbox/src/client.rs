use crate::{seccomp, Result};
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn sandbox(
    paths_ro: Option<&[&str]>,
    paths_rw: Option<&[&str]>,
    paths_x: Option<&[&str]>,
    connect_ports: Option<&[u16]>,
) -> Result<()> {
    crate::landlock(paths_ro, paths_rw, paths_x, connect_ports)?;

    let mut ctx = seccomp::new_context_with_common_rules(vec![], vec![])?;

    // socket AF_UNIX
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::socket,
        &[Comparator::new(0, Cmp::Eq, libc::AF_UNIX as u64, None)],
    )?;
    // socket NETLINK
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::socket,
        &[Comparator::new(0, Cmp::Eq, libc::AF_NETLINK as u64, None)],
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

    ctx.allow_syscall(Syscall::bind)?;
    ctx.allow_syscall(Syscall::clone3)?;
    ctx.allow_syscall(Syscall::close)?;
    ctx.allow_syscall(Syscall::connect)?;
    ctx.allow_syscall(Syscall::epoll_ctl)?;

    #[cfg(not(target_arch = "aarch64"))]
    ctx.allow_syscall(Syscall::epoll_wait)?;
    ctx.allow_syscall(Syscall::epoll_pwait)?;

    ctx.allow_syscall(Syscall::exit)?;
    ctx.allow_syscall(Syscall::fcntl)?;
    ctx.allow_syscall(Syscall::fstat)?;
    ctx.allow_syscall(Syscall::ftruncate)?;
    ctx.allow_syscall(Syscall::getdents64)?;
    ctx.allow_syscall(Syscall::getcwd)?;
    ctx.allow_syscall(Syscall::getpid)?;
    ctx.allow_syscall(Syscall::getrandom)?;
    ctx.allow_syscall(Syscall::getsockname)?;
    ctx.allow_syscall(Syscall::ioctl)?;
    ctx.allow_syscall(Syscall::lseek)?;
    ctx.allow_syscall(Syscall::madvise)?;
    ctx.allow_syscall(Syscall::newfstatat)?;
    ctx.allow_syscall(Syscall::openat)?;

    #[cfg(not(target_arch = "aarch64"))]
    ctx.allow_syscall(Syscall::poll)?;
    ctx.allow_syscall(Syscall::ppoll)?;

    ctx.allow_syscall(Syscall::prlimit64)?;
    ctx.allow_syscall(Syscall::read)?;

    #[cfg(not(target_arch = "aarch64"))]
    ctx.allow_syscall(Syscall::readlink)?;
    ctx.allow_syscall(Syscall::readlinkat)?;

    ctx.allow_syscall(Syscall::recvfrom)?;
    ctx.allow_syscall(Syscall::recvmsg)?;
    ctx.allow_syscall(Syscall::rseq)?;
    ctx.allow_syscall(Syscall::sched_getaffinity)?;
    ctx.allow_syscall(Syscall::sched_setaffinity)?;
    ctx.allow_syscall(Syscall::sched_setscheduler)?;
    ctx.allow_syscall(Syscall::sched_yield)?;
    ctx.allow_syscall(Syscall::sendmsg)?;
    ctx.allow_syscall(Syscall::sendto)?;
    ctx.allow_syscall(Syscall::set_robust_list)?;
    ctx.allow_syscall(Syscall::setpriority)?;
    ctx.allow_syscall(Syscall::shutdown)?;
    ctx.allow_syscall(Syscall::statfs)?;
    ctx.allow_syscall(Syscall::statx)?;
    ctx.allow_syscall(Syscall::timerfd_settime)?;
    ctx.allow_syscall(Syscall::uname)?;
    ctx.allow_syscall(Syscall::write)?;
    ctx.allow_syscall(Syscall::writev)?;

    ctx.load()?;

    Ok(())
}
