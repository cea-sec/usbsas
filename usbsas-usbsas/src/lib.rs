pub mod children;
use crate::children::Children;
pub mod filter;
pub mod states;

use std::{
    collections::{BTreeMap, HashMap},
    env,
};
use usbsas_proto::common::{
    device::Device, AnalyzeReport, FileInfoReport, FileType, FsType, TransferReport, UsbDevice,
};

type Devices = HashMap<u64, Device>;

#[derive(Debug, Clone, Default)]
struct TransferFiles {
    files: BTreeMap<String, FileInfo>,
    filtered: Vec<String>,
    errors: Vec<String>,
}

impl TransferFiles {
    fn new() -> Self {
        TransferFiles {
            files: BTreeMap::new(),
            filtered: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn add(&mut self, path: &str, fi: FileInfo) {
        self.files.insert(path.to_string(), fi);
    }

    fn contains(&self, path: &str) -> bool {
        self.files.contains_key(path)
            || self.errors.contains(&path.to_string())
            || self.filtered.contains(&path.to_string())
    }

    fn iter(&self) -> std::collections::btree_map::Iter<'_, std::string::String, FileInfo> {
        self.files.iter()
    }

    fn set_error(&mut self, path: &str) {
        if let Some(fileinfo) = self.files.get_mut(path) {
            fileinfo.status = FileStatus::Error;
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    size: u64,
    ftype: FileType,
    timestamp: i64,
    status: FileStatus,
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

#[derive(Debug, Clone, PartialEq)]
enum FileStatus {
    Unknown,
    Clean,
    Dirty,
    Error,
}

#[derive(Debug)]
struct Transfer {
    src: Device,
    dst: Device,
    userid: String,
    outfstype: Option<FsType>,
    max_dst_size: Option<u64>,
    selected_size: Option<u64>,
    analyze: bool,
    files: TransferFiles,
    analyze_report: Option<AnalyzeReport>,
}

impl Transfer {
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
    let mut files: BTreeMap<String, FileInfoReport> = BTreeMap::new();
    let (errors, filtered, rejected) = if let Some(tfiles) = transfer_files {
        tfiles
            .files
            .iter()
            .filter(|(_, fi)| {
                fi.ftype == FileType::Regular
                    && (fi.status != FileStatus::Dirty && fi.status != FileStatus::Error)
            })
            .for_each(|(path, fi)| {
                let _ = files.insert(
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
        )
    } else {
        (vec![], vec![], vec![])
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
        files,
        errors,
        filtered,
        rejected,
        analyzereport,
    }
}

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
