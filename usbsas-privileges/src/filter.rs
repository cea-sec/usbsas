use crate::Result;
use std::os::unix::io::RawFd;
use syscallz::Syscall;

pub fn drop_priv(fd_read: RawFd, fd_write: RawFd) -> Result<()> {
    let mut ctx = crate::new_context_with_common_rules(vec![fd_read], vec![fd_write])?;

    // Needed by toml::from_str() apparently
    ctx.allow_syscall(Syscall::getrandom)?;

    ctx.load()?;

    Ok(())
}
