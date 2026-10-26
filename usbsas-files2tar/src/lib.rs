//! usbsas's files2tar process. files2tar writes files in a tar archive.
//!
//! Transferred files are stored under a `data/` directory, metadata files
//! (like the `config.json` report with information about the transfer) at the
//! root of the archive.
//!
//! States: `Init` (open archive, sandbox) -> `WaitNewFile` <-> `WritingFile`
//! -> `WaitEnd` (after `Close`) -> `End`.

use thiserror::Error;
use usbsas_proto::common::FileType;

mod files2tar;
mod tarwriter;

pub use crate::files2tar::Files2Tar;

/// Errors of this process.
#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("system time: {0}")]
    SystemTime(#[from] std::time::SystemTimeError),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
/// Result type of this process.
pub type Result<T> = std::result::Result<T, Error>;

/// Archive format written by files2tar.
pub(crate) trait ArchiveWriter {
    /// Initialize the archive (e.g. create the data directory).
    fn init(&mut self) -> Result<()>;
    /// Start a new file or directory.
    fn newfile(&mut self, path: &str, ftype: FileType, size: u64, timestamp: i64) -> Result<()>;
    /// Append data to the current file.
    fn writefile(&mut self, data: &[u8]) -> Result<()>;
    /// Finish the current file, `len_written` bytes were written.
    fn endfile(&mut self, len_written: usize) -> Result<()>;
    /// Finish and flush the archive.
    fn finish(self: Box<Self>) -> Result<()>;
}
