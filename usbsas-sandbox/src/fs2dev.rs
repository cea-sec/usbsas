use crate::{seccomp, Result};
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub fn seccomp(
    fd_read: RawFd,
    fd_write: RawFd,
    out_fs_fd: Option<RawFd>,
    device_fd: Option<RawFd>,
) -> Result<()> {
    let mut fds_read = vec![fd_read];
    if let Some(fd) = out_fs_fd {
        fds_read.push(fd);
    }
    let mut ctx = seccomp::new_context_with_common_rules(fds_read, vec![fd_write])?;

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

    if let Some(fd) = device_fd {
        seccomp::apply_libusb_rules(&mut ctx, fd)?;
    }

    ctx.load()?;

    Ok(())
}
