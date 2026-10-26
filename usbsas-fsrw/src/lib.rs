//! File systems parsing for usbsas.
//!
//! [`FSRead`] is implemented by the readers of the source file systems (FAT,
//! exFAT, NTFS, ext4, ISO9660), [`FSWrite`] by the writers of the destination
//! file systems (FAT, exFAT, NTFS).

use std::io::{Seek, Write};
use thiserror::Error;
use usbsas_proto::common::{FileInfo, FileType, FsType};

/// ext4 reader
pub mod ext4fs;
pub mod fat;
/// exFAT reader and writer (FatFs)
pub mod ff;
/// ISO9660 reader (with Joliet extensions)
pub mod iso9660;
/// NTFS reader (ntfs crate) and writer (ntfs-3g)
pub mod ntfs;

/// Errors of this crate.
#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("ntfs error: {0}")]
    Ntfs(#[from] ::ntfs::NtfsError),
    #[error("ext4 error: {0}")]
    Ext4(#[from] ext4_view::Ext4Error),
    #[error("iso error: {0}")]
    Iso9660(#[from] hadris_iso::Error),
    #[error("{0}")]
    Error(String),
    #[error("{0}")]
    FSError(String),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Error {
        Error::Error(format!("{err}"))
    }
}
pub(crate) type Result<T> = std::result::Result<T, Error>;

/// `Write + Seek`, for the files returned by [`FSWrite::newfile`].
pub trait WriteSeek: Write + Seek {}
impl<T: Write + Seek> WriteSeek for T {}

/// File system reader.
pub trait FSRead<T> {
    /// Mount the file system from `reader`.
    fn new(reader: T, sector_size: u32) -> Result<Self>
    where
        Self: Sized;
    /// Get the type, size (bytes) and timestamp (Unix, seconds) of a file.
    fn get_attr(&mut self, path: &str) -> Result<(FileType, u64, i64)>;
    /// List the entries of a directory.
    fn read_dir(&mut self, path: &str) -> Result<Vec<FileInfo>>;
    /// Read `bytes_to_read` bytes of a file at `offset` into `buf` (which must
    /// be large enough). Returns the number of bytes read, less than asked at
    /// the end of the file.
    fn read_file(
        &mut self,
        path: &str,
        buf: &mut Vec<u8>,
        offset: u64,
        bytes_to_read: u64,
    ) -> Result<u64>;
    /// Unmount the file system and return the reader.
    fn unmount_fs(self: Box<Self>) -> Result<T>;
}

/// File system writer, creating a new file system.
pub trait FSWrite<T> {
    /// Create a new file system on `fs_file`, `sector_count` sectors of
    /// `sector_size` bytes.
    // fstype as option because some implementor of this trait can handle multiple formats
    // (eg fat/exfat for ff)
    fn mkfs(
        fs_file: T,
        sector_size: u64,
        sector_count: u64,
        fstype: Option<FsType>,
    ) -> Result<Self>
    where
        Self: Sized;
    /// Create a file, its parent directory must exist.
    fn newfile(&mut self, path: &str, timestamp: i64) -> Result<Box<dyn WriteSeek + '_>>;
    /// Create a directory, its parent directory must exist.
    fn newdir(&mut self, path: &str, timestamp: i64) -> Result<()>;
    /// Remove a file (e.g. partially written).
    fn removefile(&mut self, path: &str) -> Result<()>;
    /// Set the modification time of a file.
    // Setting timestamp can be handled by this fn or directly when creating a file or a dir.
    fn settimestamp(&mut self, path: &str, timestamp: i64) -> Result<()>;
    /// Flush and unmount the file system, return the underlying stream.
    fn unmount_fs(self: Box<Self>) -> Result<T>;
}
