//! Sandboxing helpers for usbsas processes.

pub mod client;
pub mod dev2scsi;
pub mod files2fs;
pub mod files2tar;
pub mod fs2dev;
pub mod fswriter;
pub mod hiduser;
pub mod identificator;
pub mod imager;
pub mod jsonparser;
pub mod net;
pub mod scsi2files;
pub mod tar2files;
pub mod usbdev;
pub mod usbsas;

pub(crate) mod seccomp;

use landlock::{
    path_beneath_rules, Access, AccessFs, AccessNet, CompatLevel, Compatible, NetPort, Ruleset,
    RulesetAttr, RulesetCreatedAttr, RulesetStatus,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("syscallz error: {0}")]
    Syscallz(#[from] syscallz::Error),
    #[error("landlock error: {0}")]
    Landlock(#[from] landlock::RulesetError),
    #[error("procfs error: {0}")]
    Procfs(#[from] procfs::ProcError),
    #[error("{0}")]
    Error(String),
}
type Result<T> = std::result::Result<T, Error>;

/* XXX: Functions returning constants we need and can't (yet) get from bindgen
 * because it cannot develop macro.
 * see: https://github.com/rust-lang/rust-bindgen/issues/753
 */
extern "C" {
    pub fn usbdevfs_submiturb() -> u64;
    pub fn usbdevfs_reapurbndelay() -> u64;
    pub fn usbdevfs_releaseinterface() -> u64;
    pub fn usbdevfs_ioctl() -> u64;
    pub fn usbdevfs_discardurb() -> u64;
    pub fn usbdevfs_get_capabilities() -> u64;
    pub fn usbdevfs_disconnect_claim() -> u64;
    pub fn usbdevfs_reset() -> u64;
}

pub fn landlock(
    paths_ro: Option<&[&str]>,
    paths_rw: Option<&[&str]>,
    paths_x: Option<&[&str]>,
    connect_ports: Option<&[u16]>,
) -> Result<()> {
    #[cfg(not(feature = "landlock-enforce"))]
    let ruleset = Ruleset::default().set_compatibility(CompatLevel::BestEffort);

    #[cfg(feature = "landlock-enforce")]
    let ruleset = Ruleset::default().set_compatibility(CompatLevel::HardRequirement);

    let mut ruleset = ruleset
        .handle_access(AccessFs::from_all(landlock::ABI::V2))?
        .handle_access(AccessNet::from_all(landlock::ABI::V4))?
        .create()?;

    if let Some(paths) = paths_ro {
        ruleset = ruleset.add_rules(path_beneath_rules(
            paths,
            AccessFs::from_read(landlock::ABI::V2),
        ))?;
    }

    if let Some(paths) = paths_rw {
        ruleset = ruleset.add_rules(path_beneath_rules(
            paths,
            AccessFs::from_all(landlock::ABI::V2),
        ))?;
    }

    if let Some(paths) = paths_x {
        ruleset = ruleset.add_rules(path_beneath_rules(paths, AccessFs::Execute))?;
    }

    if let Some(ports) = connect_ports {
        for port in ports {
            ruleset = ruleset.add_rule(NetPort::new(*port, AccessNet::ConnectTcp))?;
        }
    };

    let status = ruleset.restrict_self()?;

    match status.ruleset {
        RulesetStatus::FullyEnforced => {
            log::debug!("landlock enforced");
        }
        RulesetStatus::PartiallyEnforced | RulesetStatus::NotEnforced => {
            log::warn!("landlock not fully enforced: {:?}", status.ruleset);
        }
    }
    Ok(())
}
