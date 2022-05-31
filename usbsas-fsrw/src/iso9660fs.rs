use crate::FSRead;
use crate::{Error, Result};
use std::io::{Read, Seek, SeekFrom};
use usbsas_proto::common::{FileInfo, FileType};

use iso9660::{DirectoryEntry, ISO9660Reader, ISODirectory, ISOFile, ISO9660};

pub struct Iso9660<T: Read + Seek> {
    fs: ISO9660<T>,
}

fn isobject_from_path<T: ISO9660Reader>(fs: &ISO9660<T>, path: &str) -> Result<DirectoryEntry<T>> {
    let split_path = path
        .trim_start_matches('/')
        .trim_end_matches('/')
        .split('/');
    let mut cur_file = DirectoryEntry::Directory(fs.root.clone());
    for cur_path in split_path {
        // Already opened root dir
        if cur_path.is_empty() {
            continue;
        }
        cur_file = match cur_file {
            DirectoryEntry::Directory(dir) => match dir.find(cur_path)? {
                Some(new_file) => new_file,
                None => return Err(Error::Error("couldn't find path".to_string())),
            },
            _ => return Err(Error::Error("isobject_from_path".to_string())),
        }
    }
    Ok(cur_file)
}

fn isodir_from_path<T: ISO9660Reader>(fs: &ISO9660<T>, path: &str) -> Result<ISODirectory<T>> {
    match isobject_from_path(fs, path)? {
        DirectoryEntry::Directory(dir) => Ok(dir),
        _ => Err(Error::Error("isodir_from_path".to_string())),
    }
}

fn isofile_from_path<T: ISO9660Reader>(fs: &ISO9660<T>, path: &str) -> Result<ISOFile<T>> {
    match isobject_from_path(fs, path)? {
        DirectoryEntry::File(file) => Ok(file),
        _ => Err(Error::Error("isofile_from_path".to_string())),
    }
}

impl<T: Read + Seek> FSRead<T> for Iso9660<T> {
    fn new(reader: T, _sector_size: u32) -> Result<Self> {
        let fs = ISO9660::new(reader)?;
        Ok(Iso9660 { fs })
    }

    fn get_attr(&mut self, path: &str) -> Result<(FileType, u64, i64)> {
        log::trace!("get_attr: '{}'", path);
        let (ftype, size, tm) = match isobject_from_path(&self.fs, path)? {
            DirectoryEntry::Directory(dir) => (FileType::Directory, 0, dir.time()),
            DirectoryEntry::File(file) => (FileType::Regular, file.size() as u64, file.time()),
        };
        let ts = time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(
                1980 + tm.tm_year,
                time::Month::try_from(tm.tm_mon as u8).unwrap_or(time::Month::January),
                tm.tm_mday as u8,
            )
            .unwrap_or_else(|_| {
                time::Date::from_calendar_date(1980, time::Month::January, 1).unwrap()
            }),
            time::Time::from_hms(tm.tm_hour as u8, tm.tm_min as u8, tm.tm_sec as u8)
                .unwrap_or_else(|_| time::Time::from_hms(0, 0, 0).unwrap()),
        )
        .assume_utc()
        .unix_timestamp();
        Ok((ftype, size, ts))
    }

    fn read_dir(&mut self, path: &str) -> Result<Vec<FileInfo>> {
        log::trace!("read_dir: '{}'", path);
        let dir = isodir_from_path(&self.fs, path)?;
        let mut entries: Vec<FileInfo> = Vec::new();
        for entry in dir.contents() {
            let name_str = entry?.identifier().to_string();
            if !(name_str == "." || name_str == "..") {
                let full_name = format!("{}/{}", path, name_str);
                let (ftype, size, timestamp) = self.get_attr(&full_name)?;
                entries.push(FileInfo {
                    path: full_name,
                    size,
                    ftype: ftype.into(),
                    timestamp,
                });
            }
        }
        Ok(entries)
    }

    fn read_file(
        &mut self,
        path: &str,
        buf: &mut Vec<u8>,
        offset: u64,
        bytes_to_read: u64,
    ) -> Result<u64> {
        log::trace!("read_file: '{}'", path);
        let file = isofile_from_path(&self.fs, path)?;
        let mut file_reader = file.read();
        file_reader.seek(SeekFrom::Start(offset))?;

        // read() while buffer isn't full or EOF is reached.
        // don't use read_exact() because it whould ret an error and fuse always asks 4kb
        let mut bytes_read = 0;
        loop {
            match file_reader.read(&mut buf[bytes_read..]) {
                Ok(size) => {
                    bytes_read += size;
                    if bytes_read as u64 == bytes_to_read || size == 0 {
                        return Ok(bytes_read as u64);
                    }
                }
                Err(err) => {
                    log::error!("read error: {}", err);
                    return Err(err.into());
                }
            }
        }
    }

    fn unmount_fs(self: Box<Self>) -> Result<T> {
        Ok(self.fs.try_into_inner()?)
    }
}
