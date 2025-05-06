use crate::{Error, Result};
use crate::{FSRead, FSWrite, WriteSeek};
use std::{
    convert::TryFrom,
    io::{Read, Seek, Write},
};
use usbsas_proto::common::{FileInfo, FileType, FsType};

pub struct FatFsReader<T> {
    fs: ff::FatFs<T>,
}

impl<T: Read + Seek> FSRead<T> for FatFsReader<T> {
    fn new(reader: T, sector_size: u32) -> Result<Self> {
        let fs = ff::FatFs::new(reader, sector_size)?;
        Ok(FatFsReader { fs })
    }

    fn get_attr(&mut self, path: &str) -> Result<(FileType, u64, i64)> {
        log::trace!("get attr {}", path);
        if path.is_empty() || path == "/" {
            // can't stat root dir
            return Ok((FileType::Directory, 0, 0));
        }
        let file_info = self
            .fs
            .get_attr(path)
            .map_err(|err| Error::FSError(format!("Couldn't get attr for {path}: {err}")))?;
        if file_info.is_dir() {
            Ok((FileType::Directory, file_info.size, file_info.timestamp))
        } else {
            Ok((FileType::Regular, file_info.size, file_info.timestamp))
        }
    }

    fn read_dir(&mut self, path: &str) -> Result<Vec<FileInfo>> {
        log::trace!("readdir {}", path);
        Ok(self
            .fs
            .read_dir(path)
            .map_err(|err| Error::FSError(format!("Couldn't read dir {path}: {err}")))?
            .iter()
            .map(|x| FileInfo {
                path: format!("{}/{}", path.trim_end_matches('/'), x.name),
                size: x.size,
                timestamp: x.timestamp,
                ftype: if x.is_dir() {
                    FileType::Directory.into()
                } else {
                    FileType::Regular.into()
                },
            })
            .collect())
    }

    fn read_file(
        &mut self,
        path: &str,
        buf: &mut Vec<u8>,
        offset: u64,
        bytes_to_read: u64,
    ) -> Result<u64> {
        log::trace!("read_file {}", path);
        self.fs
            .read_file(path, buf, offset, bytes_to_read)
            .map_err(|err| err.into())
    }

    fn unmount_fs(self: Box<Self>) -> Result<T> {
        log::trace!("unmount_fs");
        Ok(self.fs.into_inner_r()?)
    }
}

pub struct FatFsWriter<T> {
    fs: ff::FatFs<T>,
}

impl<T: Read + Write + Seek> FSWrite<T> for FatFsWriter<T> {
    fn mkfs(writer: T, sector_size: u64, sector_count: u64, fstype: Option<FsType>) -> Result<Self>
    where
        Self: Sized,
    {
        let fstype = match fstype {
            Some(FsType::Exfat) => ff::FM_EXFAT as u8,
            Some(FsType::Fat) => ff::FM_FAT32 as u8,
            _ => return Err(Error::FSError("ff unsupported fstype".into())),
        };
        Ok(FatFsWriter {
            fs: ff::FatFs::mkfs(
                writer,
                u32::try_from(sector_size)?,
                u32::try_from(sector_count)?,
                fstype,
            )?,
        })
    }

    fn newfile(&mut self, path: &str, _timestamp: i64) -> Result<Box<dyn WriteSeek + '_>> {
        log::trace!("new file {}", path);
        Ok(Box::new(self.fs.new_file(path).map_err(|err| {
            Error::FSError(format!("Couldn't create file {path}: {err}"))
        })?))
    }

    fn newdir(&mut self, path: &str, timestamp: i64) -> Result<()> {
        log::trace!("new dir: {}", path);
        self.fs
            .new_dir(path)
            .map_err(|err| Error::FSError(format!("Couldn't create dir {path}: {err}")))?;
        self.settimestamp(path, timestamp)?;
        Ok(())
    }

    fn removefile(&mut self, path: &str) -> Result<()> {
        log::trace!("rm file {}", path);
        self.fs
            .remove_file(path)
            .map_err(|err| Error::FSError(format!("Couldn't rm file {path}: {err}")))?;
        Ok(())
    }

    fn settimestamp(&mut self, path: &str, timestamp: i64) -> Result<()> {
        log::trace!("set timestamp");
        self.fs
            .set_timestamp(path, timestamp)
            .map_err(|err| Error::FSError(format!("Couldn't rm file {path}: {err}")))?;
        Ok(())
    }

    fn unmount_fs(self: Box<Self>) -> Result<T> {
        log::trace!("unmount_fs");
        Ok(self.fs.into_inner_rw()?)
    }
}
