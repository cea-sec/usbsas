use crate::Result;
use std::os::unix::io::RawFd;

pub fn drop_priv(fd_read: RawFd, fd_write: RawFd) -> Result<()> {
    let ctx = crate::new_context_with_common_rules(vec![fd_read], vec![fd_write])?;

    ctx.load()?;

    Ok(())
}
