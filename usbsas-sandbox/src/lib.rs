//! Sandboxing helpers for usbsas processes.
//!
//! Each process has its own module applying its seccomp filter (and landlock
//! rules if needed). Filters start from common rules (read / write on the
//! given file descriptors, memory allocation without `PROT_EXEC`, logging on
//! stderr...) and kill the process on any other syscall.
//!
//! Processes open the files they need first, then apply their filter before
//! parsing any untrusted data.

/// Sandbox of the GUI client (landlock + seccomp).
pub mod client;
/// seccomp filter of dev2scsi.
pub mod dev2scsi;
/// seccomp filter of files2fs.
pub mod files2fs;
/// seccomp filter of files2tar.
pub mod files2tar;
/// seccomp filter of fs2dev.
pub mod fs2dev;
/// seccomp filter of the fswriter tool.
pub mod fswriter;
/// seccomp filter of hid-user.
pub mod hiduser;
/// seccomp filter of the identifier.
pub mod identifier;
/// seccomp filter of the imager tool.
pub mod imager;
/// seccomp filter of the jsonparser.
pub mod jsonparser;
/// seccomp filter of local2files.
pub mod local2files;
/// Sandbox of the network processes (analyzer, downloader, uploader).
pub mod net;
/// seccomp filter of scsi2files.
pub mod scsi2files;
/// seccomp filter of tar2files.
pub mod tar2files;
/// seccomp filters of usbdev (main thread and udev thread).
pub mod usbdev;
/// Sandbox of the usbsas server.
pub mod usbsas;

pub(crate) mod seccomp;

use landlock::{
    Access, AccessFs, AccessNet, CompatLevel, Compatible, NetPort, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus, path_beneath_rules,
};
use thiserror::Error;

/// Errors of this crate.
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
// `USBDEVFS_*` ioctl numbers, allowed for processes using libusb.
unsafe extern "C" {
    pub fn usbdevfs_submiturb() -> u64;
    pub fn usbdevfs_reapurbndelay() -> u64;
    pub fn usbdevfs_releaseinterface() -> u64;
    pub fn usbdevfs_ioctl() -> u64;
    pub fn usbdevfs_discardurb() -> u64;
    pub fn usbdevfs_get_capabilities() -> u64;
    pub fn usbdevfs_disconnect_claim() -> u64;
    pub fn usbdevfs_reset() -> u64;
}

/// Restrict file system and network access of the process with landlock.
///
/// - `paths_ro`: read only access,
/// - `paths_rw`: full access,
/// - `paths_x`: execution,
/// - `paths_rm`: create and remove regular files,
/// - `connect_ports`: allowed TCP ports for `connect()`.
///
/// Everything else is denied. Unless the `landlock-enforce` feature is
/// enabled, rules are applied as much as the kernel supports (best effort) and
/// a warning is logged if they can't be fully enforced.
pub fn landlock(
    paths_ro: Option<&[&str]>,
    paths_rw: Option<&[&str]>,
    paths_x: Option<&[&str]>,
    paths_rm: Option<&[&str]>,
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

    if let Some(paths) = paths_rm {
        ruleset = ruleset.add_rules(path_beneath_rules(
            paths,
            AccessFs::RemoveFile | AccessFs::MakeReg,
        ))?;
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
