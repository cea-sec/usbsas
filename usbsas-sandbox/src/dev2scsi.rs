use crate::{seccomp, Result};
use std::os::unix::io::RawFd;

pub fn seccomp(fd_read: RawFd, fd_write: RawFd, device_fd: Option<RawFd>) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(vec![fd_read], vec![fd_write])?;

    if let Some(device_fd) = device_fd {
        seccomp::apply_libusb_rules(&mut ctx, device_fd)?;
    }

    ctx.load()?;

    Ok(())
}
