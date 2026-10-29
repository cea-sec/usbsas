//! usbsas server, the parent process orchestrating every other usbsas process.
//!
//! The server is implemented as a state machine (see [`states`]): each state
//! handles requests from the client and drives the children (see
//! [`children`]) accordingly. This module contains the bookkeeping of a
//! transfer and the generation of its report.

pub mod children;
use crate::children::Children;
pub mod filter;
pub mod states;

use std::{
    collections::{BTreeMap, HashMap},
    env, fs,
};
use usbsas_proto::common::{
    AnalyzeReport, FileInfoReport, FileType, FsType, TransferReport, UsbDevice, device::Device,
};

/// Available devices (USB, network, local directories, command), indexed by
/// their id.
type Devices = HashMap<u64, Device>;

/// Files of a transfer.
#[derive(Debug, Clone, Default)]
struct TransferFiles {
    /// Files and directories to transfer, indexed by path
    files: BTreeMap<String, FileInfo>,
    /// Paths excluded by the configured filters
    filtered: Vec<String>,
    /// Paths that couldn't be read
    errors: Vec<String>,
    /// Directories at the maximum depth whose content was skipped (or paths
    /// deeper than the maximum depth)
    depth_exceeded: Vec<String>,
}

impl TransferFiles {
    fn new() -> Self {
        TransferFiles {
            files: BTreeMap::new(),
            filtered: Vec::new(),
            errors: Vec::new(),
            depth_exceeded: Vec::new(),
        }
    }

    fn add(&mut self, path: &str, fi: FileInfo) {
        self.files.insert(path.to_string(), fi);
    }

    /// Whether `path` was already processed (added, filtered or in error).
    fn contains(&self, path: &str) -> bool {
        self.files.contains_key(path)
            || self.errors.contains(&path.to_string())
            || self.filtered.contains(&path.to_string())
            || self.depth_exceeded.contains(&path.to_string())
    }

    fn iter(&self) -> std::collections::btree_map::Iter<'_, std::string::String, FileInfo> {
        self.files.iter()
    }

    /// Mark a file as failed during the transfer.
    fn set_error(&mut self, path: &str) {
        if let Some(fileinfo) = self.files.get_mut(path) {
            fileinfo.status = FileStatus::Error;
        }
    }
}

/// Depth of a path
fn path_depth(path: &str) -> usize {
    path.split('/').filter(|part| !part.is_empty()).count()
}

/// Metadata of a file of the transfer.
#[derive(Debug, Clone)]
pub struct FileInfo {
    size: u64,
    ftype: FileType,
    timestamp: i64,
    status: FileStatus,
    /// SHA-256 of the file content (hex), computed when read from the source
    cksum: Option<String>,
}

impl From<&usbsas_proto::common::FileInfo> for FileInfo {
    fn from(fi: &usbsas_proto::common::FileInfo) -> Self {
        Self {
            size: fi.size,
            timestamp: fi.timestamp,
            status: FileStatus::Unknown,
            ftype: FileType::try_from(fi.ftype).unwrap_or(FileType::Other),
            cksum: None,
        }
    }
}

impl From<&usbsas_proto::files::ResponseGetAttr> for FileInfo {
    fn from(attrs: &usbsas_proto::files::ResponseGetAttr) -> Self {
        Self {
            size: attrs.size,
            timestamp: attrs.timestamp,
            status: FileStatus::Unknown,
            ftype: FileType::try_from(attrs.ftype).unwrap_or(FileType::Other),
            cksum: None,
        }
    }
}

/// Status of a file, updated by the analysis and the copy.
#[derive(Debug, Clone, PartialEq)]
enum FileStatus {
    /// Not analyzed
    Unknown,
    /// Analyzed and clean
    Clean,
    /// Analyzed and dirty, won't be copied
    Dirty,
    /// Couldn't be copied
    Error,
}

/// A file transfer in progress, passed from state to state.
#[derive(Debug)]
struct Transfer {
    src: Device,
    dst: Device,
    userid: String,
    /// File system to create on a USB destination
    outfstype: Option<FsType>,
    /// Maximum size of the files that can be written on the destination
    max_dst_size: Option<u64>,
    /// Total size of the selected files (minus dirty ones after analysis)
    selected_size: Option<u64>,
    /// Whether files must be analyzed before being written
    analyze: bool,
    files: TransferFiles,
    analyze_report: Option<AnalyzeReport>,
}

impl Transfer {
    /// Build the report of this transfer with the given status.
    fn to_report(&self, status: &str) -> TransferReport {
        report(
            "usbsas_transfer",
            status,
            Some(self.userid.clone()),
            Some(&self.src),
            Some(&self.dst),
            Some(&self.files),
            self.analyze_report.clone(),
        )
    }
}

/// Build a report.
///
/// Only regular files that are neither dirty nor in error are listed in
/// `file_names` / `files_info`; dirty files are listed in `rejected`.
fn report(
    title: &str,
    status: &str,
    user: Option<String>,
    source: Option<&Device>,
    destination: Option<&Device>,
    transfer_files: Option<&TransferFiles>,
    analyzereport: Option<AnalyzeReport>,
) -> TransferReport {
    let (hostname, time, datetime) = report_infos();
    let transfer_id = env::var("USBSAS_SESSION_ID").unwrap_or("0".to_string());
    let mut files_info: BTreeMap<String, FileInfoReport> = BTreeMap::new();
    let mut file_names = Vec::new();
    let (errors, filtered, rejected, depth_exceeded) = if let Some(tfiles) = transfer_files {
        tfiles
            .files
            .iter()
            .filter(|(_, fi)| {
                fi.ftype == FileType::Regular
                    && (fi.status != FileStatus::Dirty && fi.status != FileStatus::Error)
            })
            .for_each(|(path, fi)| {
                file_names.push(path.to_string());
                let _ = files_info.insert(
                    path.to_string(),
                    FileInfoReport {
                        size: fi.size,
                        timestamp: fi.timestamp,
                        sha256: fi.cksum.clone(),
                    },
                );
            });
        let mut errors = tfiles.errors.clone();
        errors.extend_from_slice(
            &tfiles
                .files
                .iter()
                .filter_map(|(path, fi)| {
                    if fi.status == FileStatus::Error {
                        Some(path.to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<String>>(),
        );
        (
            errors,
            tfiles.filtered.clone(),
            tfiles
                .files
                .iter()
                .filter_map(|(path, fi)| {
                    if fi.status == FileStatus::Dirty {
                        Some(path.trim_start_matches('/').into())
                    } else {
                        None
                    }
                })
                .collect(),
            tfiles.depth_exceeded.clone(),
        )
    } else {
        (vec![], vec![], vec![], vec![])
    };
    TransferReport {
        title: format!("{title}_{datetime}_{transfer_id}"),
        datetime,
        hostname,
        timestamp: time.unix_timestamp(),
        transfer_id,
        status: status.into(),
        user,
        source: source.map(|x| x.into()),
        destination: destination.map(|x| x.into()),
        files_info,
        file_names,
        errors,
        filtered,
        rejected,
        analyzereport,
        depth_exceeded,
    }
}

/// Report of a disk image.
fn report_diskimg(device: UsbDevice) -> TransferReport {
    crate::report(
        "diskimg",
        "success",
        None,
        None,
        Some(&Device::Usb(device)),
        None,
        None,
    )
}

/// Report of a wipe.
fn report_wipe(device: UsbDevice) -> TransferReport {
    crate::report(
        "wipe",
        "success",
        None,
        Some(&Device::Usb(device)),
        None,
        None,
        None,
    )
}

/// Hostname, current time and its formatted version (`YYYYMMDDhhmmssms`),
/// used in reports.
fn report_infos() -> (String, time::OffsetDateTime, String) {
    #[cfg(not(feature = "integration-tests"))]
    let (hostname, time) = {
        let name = match nix::sys::utsname::uname() {
            Ok(utsname) => utsname.nodename().to_string_lossy().to_string(),
            _ => "unknown-usbsas".into(),
        };
        (name, time::OffsetDateTime::now_utc())
    };
    // Fixed values to keep a deterministic filesystem hash
    #[cfg(feature = "integration-tests")]
    let (hostname, time) = (
        "unknown-usbsas".into(),
        time::macros::datetime!(2020-01-01 0:00 UTC),
    );
    let datetime = format!(
        "{:04}{:02}{:02}{:02}{:02}{:02}{:03}",
        time.year(),
        time.month() as u8,
        time.day(),
        time.hour(),
        time.minute(),
        time.second(),
        time.millisecond()
    );
    (hostname, time, datetime)
}

/// Working files of the session, removed on drop (unless `keep`). The socket is
/// always removed.
pub struct TmpFiles {
    pub tar_path: String,
    pub clean_tar_path: String,
    pub fs_path: String,
    pub socket_path: Option<String>,
    pub keep: bool,
}

impl Drop for TmpFiles {
    fn drop(&mut self) {
        if let Some(path) = &self.socket_path {
            let _ = fs::remove_file(path);
        }
        if !self.keep {
            let _ = fs::remove_file(&self.tar_path);
            let _ = fs::remove_file(&self.clean_tar_path);
            let _ = fs::remove_file(&self.fs_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::path_depth;

    #[test]
    fn depth() {
        assert_eq!(path_depth("/"), 0);
        assert_eq!(path_depth(""), 0);
        assert_eq!(path_depth("/a"), 1);
        assert_eq!(path_depth("/a/b/c.txt"), 3);
        assert_eq!(path_depth("a/b/"), 2);
        assert_eq!(path_depth("//a//b"), 2);
    }
}
