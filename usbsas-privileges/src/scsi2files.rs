use crate::Result;
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn drop_priv(fds_read: Vec<RawFd>, fds_write: Vec<RawFd>) -> Result<()> {
    let mut ctx = crate::new_context_with_common_rules(fds_read, fds_write)?;

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

    ctx.allow_syscall(Syscall::getrandom)?;

    ctx.load()?;
    Ok(())
}
