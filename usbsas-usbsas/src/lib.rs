pub mod children;
use crate::children::Children;
pub mod filter;
pub mod states;

use std::collections::HashMap;
use thiserror::Error;
use usbsas_proto::common::{device::Device, FsType};

#[derive(Error, Debug)]
enum Error {
    #[error("Unexpected request")]
    BadRequest,
    #[error("Not enough space on destination device")]
    NotEnoughSpace,
    #[error("{0}")]
    Error(String),
}

type Devices = HashMap<u64, Device>;
type TransferReport = serde_json::Value;

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

struct Transfer {
    src: Device,
    dst: Device,
    userid: String,
    outfstype: Option<FsType>,
    max_dst_size: Option<u64>,
    selected_size: Option<u64>,
    analyze: bool,
    files: TransferFiles,
    report: TransferReport,
}
