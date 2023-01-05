use crate::Result;
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn drop_priv(fd_read: RawFd, fd_write: RawFd, in_tar_fd: Option<RawFd>) -> Result<()> {
    let mut fds_read = vec![fd_read];
    if let Some(fd) = in_tar_fd {
        fds_read.push(fd);
    }
    let mut ctx = crate::new_context_with_common_rules(fds_read, vec![fd_write])?;

    if let Some(fd) = in_tar_fd {
        // Allow lseek on tar
        ctx.set_rule_for_syscall(
            Action::Allow,
            #[cfg(not(target_arch = "arm"))]
            Syscall::lseek,
            #[cfg(target_arch = "arm")]
            Syscall::_llseek,
            &[Comparator::new(0, Cmp::Eq, fd as u64, None)],
        )?;
    }

    ctx.allow_syscall(Syscall::getrandom)?;

    ctx.load()?;
    Ok(())
}
