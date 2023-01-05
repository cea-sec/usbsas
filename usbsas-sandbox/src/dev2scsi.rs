use crate::{seccomp, Result};
use std::os::unix::io::RawFd;

pub fn seccomp(fd_read: RawFd, fd_write: RawFd, libusb_fds: crate::LibusbFds) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(vec![fd_read], vec![fd_write])?;

    seccomp::apply_libusb_rules(&mut ctx, libusb_fds)?;

    ctx.load()?;

    Ok(())
}
