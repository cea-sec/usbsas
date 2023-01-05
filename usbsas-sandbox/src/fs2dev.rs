use crate::Result;
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn drop_priv(
    fd_read: RawFd,
    fd_write: RawFd,
    out_fs_fd: Option<RawFd>,
    libusb_fds: crate::LibusbFds,
) -> Result<()> {
    let mut fds_read = vec![fd_read];
    if let Some(fd) = out_fs_fd {
        fds_read.push(fd);
    }
    let mut ctx = crate::new_context_with_common_rules(fds_read, vec![fd_write])?;

    if let Some(fd) = out_fs_fd {
        // Allow lseek on out_fs
        ctx.set_rule_for_syscall(
            Action::Allow,
            #[cfg(not(target_arch = "arm"))]
            Syscall::lseek,
            #[cfg(target_arch = "arm")]
            Syscall::_llseek,
            &[Comparator::new(0, Cmp::Eq, fd as u64, None)],
        )?;
    }

    crate::apply_libusb_rules(&mut ctx, libusb_fds)?;

    ctx.load()?;

    Ok(())
}
