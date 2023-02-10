//! Sandboxing helpers for usbsas processes.

pub mod dev2scsi;
pub mod files2fs;
pub mod files2tar;
pub mod filter;
pub mod fs2dev;
pub mod fswriter;
pub mod identificator;
pub mod imager;
pub mod scsi2files;
pub mod tar2files;
pub mod usbdev;
pub mod usbsas;

pub(crate) mod seccomp;

use landlock::{
    path_beneath_rules, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
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

const LLABI: landlock::ABI = landlock::ABI::V1;

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

pub fn landlock(paths_ro: Option<&[&str]>, paths_rw: Option<&[&str]>) -> Result<()> {
    let mut ruleset = Ruleset::new()
        .handle_access(AccessFs::from_all(crate::LLABI))?
        .create()?
        .set_no_new_privs(true);

    if let Some(paths) = paths_ro {
        ruleset =
            ruleset.add_rules(path_beneath_rules(paths, AccessFs::from_read(crate::LLABI)))?;
    }

    if let Some(paths) = paths_rw {
        ruleset = ruleset.add_rules(path_beneath_rules(paths, AccessFs::from_all(crate::LLABI)))?;
    }

    let status = ruleset.restrict_self()?;

    match status.ruleset {
        RulesetStatus::FullyEnforced => Ok(()),
        RulesetStatus::PartiallyEnforced | RulesetStatus::NotEnforced => {
            #[cfg(feature = "landlock-enforce")]
            return Err(crate::Error::Error(
                "Couldn't fully enforce landlock".into(),
            ));
            #[cfg(not(feature = "landlock-enforce"))]
            {
                log::warn!("landlock not enforced !");
                Ok(())
            }
        }
    }
}
