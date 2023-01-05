use crate::Result;
use std::os::unix::io::RawFd;

pub fn drop_priv(fd_read: RawFd, fd_write: RawFd, libusb_fds: crate::LibusbFds) -> Result<()> {
    let mut ctx = crate::new_context_with_common_rules(vec![fd_read], vec![fd_write])?;

    crate::apply_libusb_rules(&mut ctx, libusb_fds)?;

    ctx.load()?;

    Ok(())
}
