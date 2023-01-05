use crate::{seccomp, Result};
use std::os::unix::io::RawFd;
use syscallz::Syscall;

pub fn seccomp(fds_read: Vec<RawFd>, fds_write: Vec<RawFd>) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(fds_read, fds_write)?;

    ctx.allow_syscall(Syscall::getrandom)?;

    ctx.load()?;
    Ok(())
}
