use crate::Result;
use std::os::unix::io::RawFd;
use syscallz::Syscall;

pub fn drop_priv(fds_read: Vec<RawFd>, fds_write: Vec<RawFd>) -> Result<()> {
    let mut ctx = crate::new_context_with_common_rules(fds_read, fds_write)?;

    ctx.allow_syscall(Syscall::getrandom)?;

    ctx.load()?;
    Ok(())
}
