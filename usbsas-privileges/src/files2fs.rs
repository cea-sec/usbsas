use crate::Result;
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn drop_priv(fd_read: RawFd, fd_write: RawFd, out_fs_fd: RawFd) -> Result<()> {
    let mut ctx =
        crate::new_context_with_common_rules(vec![fd_read, out_fs_fd], vec![fd_write, out_fs_fd])?;

    // Allow lseek on out_fs
    ctx.set_rule_for_syscall(
        Action::Allow,
        #[cfg(not(target_arch = "arm"))]
        Syscall::lseek,
        #[cfg(target_arch = "arm")]
        Syscall::_llseek,
        &[Comparator::new(0, Cmp::Eq, out_fs_fd as u64, None)],
    )?;

    // Allow mremap
    ctx.allow_syscall(Syscall::mremap)?;
    // but disallow with PROT_EXEC
    ctx.set_rule_for_syscall(
        Action::KillThread,
        Syscall::mremap,
        &[Comparator::new(
            2,
            Cmp::MaskedEq,
            libc::PROT_EXEC as u64,
            Some(libc::PROT_EXEC as u64),
        )],
    )?;

    ctx.load()?;

    Ok(())
}
