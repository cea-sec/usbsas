use crate::{seccomp, Result};
use landlock::{
    Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
};
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

pub struct UsbsasSocket {
    pub listen: RawFd,
    pub read: RawFd,
    pub write: RawFd,
    pub path: String,
}

pub fn sandbox(
    fds_read: Vec<RawFd>,
    fds_write: Vec<RawFd>,
    socket: Option<UsbsasSocket>,
) -> Result<()> {
    let mut ctx = seccomp::new_context_with_common_rules(fds_read, fds_write)?;

    ctx.allow_syscall(Syscall::wait4)?;
    ctx.allow_syscall(Syscall::getrandom)?;
    ctx.allow_syscall(Syscall::uname)?;

    // Allow recvfrom and close and unlink on listen fd socket
    if let Some(sock) = socket {
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::close,
            &[Comparator::new(0, Cmp::Eq, sock.listen as u64, None)],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::recvfrom,
            &[Comparator::new(0, Cmp::Eq, sock.read as u64, None)],
        )?;
        ctx.set_rule_for_syscall(
            Action::Allow,
            Syscall::sendto,
            &[Comparator::new(0, Cmp::Eq, sock.write as u64, None)],
        )?;

        // Allow unlink syscall but restrict it to socket path with landlock
        ctx.allow_syscall(Syscall::unlink)?;

        let status = Ruleset::default()
            .handle_access(AccessFs::from_all(crate::LLABI))?
            .create()?
            .set_no_new_privs(true)
            .add_rule(PathBeneath::new(
                PathFd::new(sock.path).unwrap(),
                AccessFs::from_file(crate::LLABI),
            ))?
            .restrict_self()?;

        match status.ruleset {
            RulesetStatus::FullyEnforced => (),
            RulesetStatus::PartiallyEnforced | RulesetStatus::NotEnforced => {
                #[cfg(feature = "landlock-enforce")]
                return Err(crate::Error::Error(
                    "Couldn't fully enforce landlock".into(),
                ));
                #[cfg(not(feature = "landlock-enforce"))]
                {
                    log::warn!("landlock not enforced !");
                }
            }
        }
    };

    ctx.load()?;

    Ok(())
}
