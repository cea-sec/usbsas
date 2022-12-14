//! usbsas's files2tar process. files2tar writes files in a tar archive. It can
//! be started in two modes depending on the transfer destination. If data is
//! copied to another USB device, files will be stored directly in the tar for
//! analysis. If data is uploaded to a remote server, files will be stored in
//! the tar under a "/data/" directory and a "/config.json" file containing
//! information about the input device, hostname etc. will be added.
//!

use thiserror::Error;
use usbsas_proto::common::FileType;

mod files2tar;
mod tarwriter;

pub use crate::files2tar::Files2Tar;

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("system time: {0}")]
    SystemTime(#[from] std::time::SystemTimeError),
    #[error("privileges: {0}")]
    Privileges(#[from] usbsas_privileges::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
type Result<T> = std::result::Result<T, Error>;

pub(crate) trait ArchiveWriter {
    fn init(&mut self) -> Result<()>;
    fn newfile(&mut self, path: &str, ftype: FileType, size: u64, timestamp: i64) -> Result<()>;
    fn writefile(&mut self, data: &[u8]) -> Result<()>;
    fn endfile(&mut self, len_written: usize) -> Result<()>;
    fn finish(self: Box<Self>, infos: usbsas_proto::writetar::RequestClose) -> Result<()>;
}
