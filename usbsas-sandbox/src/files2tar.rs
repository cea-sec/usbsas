use crate::{seccomp, Result};
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn seccomp(fd_read: RawFd, fd_write: RawFd, out_tar_fd: RawFd) -> Result<()> {
    let mut ctx =
        seccomp::new_context_with_common_rules(vec![fd_read], vec![fd_write, out_tar_fd])?;
    ctx.set_rule_for_syscall(
        Action::Allow,
        #[cfg(not(target_arch = "arm"))]
        Syscall::lseek,
        #[cfg(target_arch = "arm")]
        Syscall::_llseek,
        &[Comparator::new(0, Cmp::Eq, out_tar_fd as u64, None)],
    )?;
    ctx.load()?;

    Ok(())
}
