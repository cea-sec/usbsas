//! File systems parsing for usbsas.

use std::io::{Seek, Write};
use thiserror::Error;
use usbsas_proto::common::{FileInfo, FileType, OutFsType};

pub mod ext4fs;
pub mod ff;
pub mod iso9660fs;
pub mod ntfs;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("ntfs error: {0}")]
    Ntfs(#[from] ::ntfs::NtfsError),
    #[error("iso error: {0}")]
    Iso9660(#[from] iso9660::ISOError),
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
        Error::Error(format!("{}", err))
    }
}
pub(crate) type Result<T> = std::result::Result<T, Error>;

pub trait WriteSeek: Write + Seek {}
impl<T: Write + Seek> WriteSeek for T {}

pub trait FSRead<T> {
    fn new(reader: T, sector_size: u32) -> Result<Self>
    where
        Self: Sized;
    fn get_attr(&mut self, path: &str) -> Result<(FileType, u64, i64)>;
    fn read_dir(&mut self, path: &str) -> Result<Vec<FileInfo>>;
    fn read_file(
        &mut self,
        path: &str,
        buf: &mut Vec<u8>,
        offset: u64,
        bytes_to_read: u64,
    ) -> Result<u64>;
    fn unmount_fs(self: Box<Self>) -> Result<T>;
}

pub trait FSWrite<T> {
    // fstype as option because some implementor of this trait can handle multiple formats
    // (eg fat/exfat for ff)
    fn mkfs(
        fs_file: T,
        sector_size: u64,
        sector_count: u64,
        fstype: Option<OutFsType>,
    ) -> Result<Self>
    where
        Self: Sized;
    fn newfile(&mut self, path: &str, timestamp: i64) -> Result<Box<dyn WriteSeek + '_>>;
    fn newdir(&mut self, path: &str, timestamp: i64) -> Result<()>;
    fn removefile(&mut self, path: &str) -> Result<()>;
    // Setting timestamp can be handled by this fn or directly when creating a file or a dir.
    fn settimestamp(&mut self, path: &str, timestamp: i64) -> Result<()>;
    fn unmount_fs(self: Box<Self>) -> Result<T>;
}
