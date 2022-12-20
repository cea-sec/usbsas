use crate::Result;
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn drop_priv(fd_in_file: RawFd, comm_in: RawFd, comm_out: RawFd) -> Result<()> {
    let mut ctx = crate::new_context_with_common_rules(vec![fd_in_file, comm_in], vec![comm_out])?;

    // Allow lseek on out_fs
    ctx.set_rule_for_syscall(
        Action::Allow,
        #[cfg(not(target_arch = "arm"))]
        Syscall::lseek,
        #[cfg(target_arch = "arm")]
        Syscall::_llseek,
        &[Comparator::new(0, Cmp::Eq, fd_in_file as u64, None)],
    )?;

    // The following rules are for the progress bar
    // ioctl(1, TCGETS, ..)
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, 1, None),
            Comparator::new(1, Cmp::Eq, libc::TCGETS, None),
        ],
    )?;
    // ioctl(2, TCGETS, ..)
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, 2, None),
            Comparator::new(1, Cmp::Eq, libc::TCGETS, None),
        ],
    )?;
    // ioctl(2, TIOCGWINSZ, ..)
    ctx.set_rule_for_syscall(
        Action::Allow,
        Syscall::ioctl,
        &[
            Comparator::new(0, Cmp::Eq, 2, None),
            Comparator::new(1, Cmp::Eq, libc::TIOCGWINSZ, None),
        ],
    )?;

    ctx.load()?;

    Ok(())
}
