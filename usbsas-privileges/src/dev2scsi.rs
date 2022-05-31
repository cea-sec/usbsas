use crate::Result;
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn drop_priv(fd_read: RawFd, fd_write: RawFd, libusb_fds: crate::LibusbFds) -> Result<()> {
    let mut ctx = crate::new_context_with_common_rules(vec![fd_read], vec![fd_write])?;

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

    crate::apply_libusb_rules(&mut ctx, libusb_fds)?;

    ctx.load()?;

    Ok(())
}
