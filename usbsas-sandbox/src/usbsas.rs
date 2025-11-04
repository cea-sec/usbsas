use crate::{seccomp, Result};
use std::os::unix::io::RawFd;
use syscallz::{Action, Cmp, Comparator, Syscall};

use landlock::{
    make_bitflags, path_beneath_rules, Access, AccessFs, CompatLevel, Compatible, Ruleset,
    RulesetAttr, RulesetCreatedAttr, RulesetStatus,
};

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
    out_dir: &str,
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
    };

    // Allow unlink syscall but restrict it to socket path with landlock
    #[cfg(not(target_arch = "aarch64"))]
    ctx.allow_syscall(Syscall::unlink)?;
    #[cfg(target_arch = "aarch64")]
    ctx.allow_syscall(Syscall::unlinkat)?;

    #[cfg(not(feature = "landlock-enforce"))]
    let ruleset = Ruleset::default().set_compatibility(CompatLevel::BestEffort);
    #[cfg(feature = "landlock-enforce")]
    let ruleset = Ruleset::default().set_compatibility(CompatLevel::HardRequirement);
    let mut ruleset = ruleset
        .handle_access(AccessFs::from_all(landlock::ABI::V2))?
        .create()?;
    // Allow removing files
    ruleset = ruleset.add_rules(path_beneath_rules(
        &[out_dir],
        make_bitflags!(AccessFs::RemoveFile),
    ))?;
    let status = ruleset.restrict_self()?;
    match status.ruleset {
        RulesetStatus::FullyEnforced => {
            log::debug!("landlock enforced");
        }
        RulesetStatus::PartiallyEnforced | RulesetStatus::NotEnforced => {
            log::warn!("landlock not fully enforced: {:?}", status.ruleset);
        }
    }

    ctx.load()?;

    Ok(())
}
