use crate::{seccomp, Result};
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn seccomp(fds_read: Vec<RawFd>, fds_write: Vec<RawFd>) -> Result<()> {
    let mut fds_read = fds_read;
    // Allow read on stdin
    fds_read.push(0);
    let mut ctx = seccomp::new_context_with_common_rules(fds_read, fds_write)?;

    // The following rules are for the progress bar
    // ioctl(1, TCGETS, ..)
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
    // ioctl(2, TCGETS, ..)
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, 2, None),
            #[cfg(target_env = "musl")]
            Comparator::new(1, Cmp::Eq, libc::TCGETS as u64, None),
            #[cfg(not(target_env = "musl"))]
            Comparator::new(1, Cmp::Eq, libc::TCGETS, None),
        ],
    )?;
    // ioctl(2, TIOCGWINSZ, ..)
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, 2, None),
            #[cfg(target_env = "musl")]
            Comparator::new(1, Cmp::Eq, libc::TIOCGWINSZ as u64, None),
            #[cfg(not(target_env = "musl"))]
            Comparator::new(1, Cmp::Eq, libc::TIOCGWINSZ, None),
        ],
    )?;

    ctx.load()?;

    Ok(())
}
