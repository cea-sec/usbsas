pub mod children;
use crate::children::Children;
pub mod filter;
pub mod states;

use std::{collections::HashMap, env};
use usbsas_proto::common::{device::Device, AnalyzeReport, FsType, TransferReport, UsbDevice};

type Devices = HashMap<u64, Device>;

#[derive(Debug)]
struct TransferFiles {
    files: Vec<String>,
    directories: Vec<String>,
    filtered: Vec<String>,
    errors: Vec<String>,
    dirty: Vec<String>,
}

impl TransferFiles {
    fn new() -> Self {
        TransferFiles {
            files: Vec::new(),
            directories: Vec::new(),
            filtered: Vec::new(),
            errors: Vec::new(),
            dirty: Vec::new(),
        }
    }
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
            Some(self.files.files.clone()),
            Some(self.files.filtered.clone()),
            Some(self.files.errors.clone()),
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
    file_names: Option<Vec<String>>,
    filtered_files: Option<Vec<String>>,
    error_files: Option<Vec<String>>,
    analyzereport: Option<AnalyzeReport>,
) -> TransferReport {
    let (hostname, time, datetime) = report_infos();
    let transfer_id = env::var("USBSAS_SESSION_ID").unwrap_or("0".to_string());
    let rejected_files: Vec<String> = if let Some(ref report) = analyzereport {
        report
            .files
            .iter()
            .filter_map(|(file, status)| {
                if status.status == "DIRTY" {
                    Some(file.into())
                } else {
                    None
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    TransferReport {
        title: format!("{}_{}_{}", title, datetime, transfer_id),
        datetime,
        hostname,
        timestamp: time.unix_timestamp(),
        transfer_id,
        status: status.into(),
        user,
        source: source.map(|x| x.into()),
        destination: destination.map(|x| x.into()),
        file_names: file_names.unwrap_or_default(),
        error_files: error_files.unwrap_or_default(),
        filtered_files: filtered_files.unwrap_or_default(),
        rejected_files,
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
        None,
        None,
    )
}

fn report_infos() -> (String, time::OffsetDateTime, String) {
    #[cfg(not(feature = "integration-tests"))]
    let (hostname, time) = {
        let name = match uname::Info::new() {
            Ok(name) => name.nodename,
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
