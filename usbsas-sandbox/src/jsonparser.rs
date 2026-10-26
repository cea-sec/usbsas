use crate::{Result, seccomp};
use std::os::unix::io::RawFd;

/// Communication with the parent only.
pub fn seccomp(fd_read: RawFd, fd_write: RawFd) -> Result<()> {
    let ctx = seccomp::new_context_with_common_rules(vec![fd_read], vec![fd_write])?;
    ctx.load()?;
    Ok(())
}
