use crate::{Error, Result};
use crate::{FSRead, FSWrite, WriteSeek};
use ntfs::NtfsReadSeek;
use std::{
    convert::TryFrom,
    io::{Read, Seek, SeekFrom, Write},
};
use usbsas_proto::common::{FileInfo, FileType, OutFsType};

pub struct NTFS3G<T> {
    volume: ntfs3g::Ntfs3g<T>,
}

impl<T: Read + Write + Seek> FSWrite<T> for NTFS3G<T> {
    fn mkfs(
        writer: T,
        sector_size: u64,
        sector_count: u64,
        _fstype: Option<OutFsType>,
    ) -> Result<Self>
    where
        Self: Sized,
    {
        Ok(NTFS3G {
            volume: ntfs3g::Ntfs3g::mkntfs(
                writer,
                i64::try_from(sector_size)?,
                i64::try_from(sector_count)?,
            )?,
        })
    }

    fn newfile(&mut self, path: &str, timestamp: i64) -> Result<Box<dyn WriteSeek + '_>> {
        log::trace!("new file {}", path);
        let file: Box<dyn WriteSeek> =
            Box::new(self.volume.new_file(path, timestamp).map_err(|err| {
                Error::FSError(format!("Couldn't create file {}: {}", path, err))
            })?);
        Ok(file)
    }

    fn newdir(&mut self, path: &str, timestamp: i64) -> Result<()> {
        log::trace!("new dir {}", path);
        self.volume
            .new_dir(path, timestamp)
            .map_err(|err| Error::FSError(format!("Couldn't create dir {}: {}", path, err)))
    }

    fn removefile(&mut self, path: &str) -> Result<()> {
        log::trace!("remove file {}", path);
        self.volume
            .remove_file(path)
            .map_err(|err| Error::FSError(format!("Couldn't remove file {}: {}", path, err)))
    }

    fn settimestamp(&mut self, _path: &str, _timestamp: i64) -> Result<()> {
        // Timestamp is set when creating file
        Ok(())
    }

    fn unmount_fs(self: Box<Self>) -> Result<T> {
        log::trace!("unmount fs");
        Ok(self.volume.into_inner()?)
    }
}

// Use ntfs rust crate for reading fs
pub struct NTFS<T> {
    reader: T,
    fs: ntfs::Ntfs,
}

fn ntfs_file_from_path<'a, T: Read + Seek>(
    fs: &'a ntfs::Ntfs,
    reader: &mut T,
    path: &str,
) -> Result<ntfs::NtfsFile<'a>> {
    let split_path = path.trim_end_matches('/').split('/');
    let mut cur_file = fs.root_directory(reader)?;

    'outer: for cur_path in split_path {
        // Already opened root dir if path starts with a '/'
        if cur_path.is_empty() {
            continue;
        }
        let index = cur_file.directory_index(reader)?;
        let mut index_entries = index.entries();
        while let Some(entry) = index_entries.next(reader) {
            let entry = entry?.to_file(fs, reader)?;
            let name_string = ntfs_name_from_file(&entry, reader)?;
            if name_string == cur_path {
                cur_file = entry;
                continue 'outer;
            }
        }
        return Err(Error::FSError(format!(
            "didn't find file {} in {}",
            cur_path, path
        )));
    }
    Ok(cur_file)
}

fn ntfs_name_from_file<'a, T: Read + Seek>(
    file: &'a ntfs::NtfsFile,
    reader: &mut T,
) -> Result<String> {
    // Try to get the Posix name (it may not be the first registered) or
    // fallback to whatever there is.
    let name = if let Some(name) = file.name(
        reader,
        Some(ntfs::structured_values::NtfsFileNamespace::Posix),
        None,
    ) {
        name
    } else if let Some(name) = file.name(
        reader,
        Some(ntfs::structured_values::NtfsFileNamespace::Win32),
        None,
    ) {
        name
    } else if let Some(name) = file.name(
        reader,
        Some(ntfs::structured_values::NtfsFileNamespace::Win32AndDos),
        None,
    ) {
        name
    } else if let Some(name) = file.name(reader, None, None) {
        name
    } else {
        return Err(Error::FSError("Didn't find file name".to_string()));
    };

    if let Some(name_string) = name?.name().to_string_checked() {
        Ok(name_string)
    } else {
        Err(Error::FSError("Didn't find file name".to_string()))
    }
}

fn ntfs_file_size<T: Read + Seek>(ntfs_file: &ntfs::NtfsFile, reader: &mut T) -> Result<u64> {
    if ntfs_file.is_directory() {
        Ok(0)
    } else {
        // Empty string here means we ask for the unnamed $DATA attribute
        let data_item = match ntfs_file.data(reader, "") {
            Some(data_item) => data_item?,
            None => return Err(Error::FSError("No ntfs data stream".into())),
        };
        Ok(data_item.to_attribute().value(reader)?.len())
    }
}

impl<T: Read + Seek> FSRead<T> for NTFS<T> {
    fn new(mut reader: T, _sector_size: u32) -> Result<Self> {
        let fs = ntfs::Ntfs::new(&mut reader)?;
        Ok(NTFS { reader, fs })
    }

    fn get_attr(&mut self, path: &str) -> Result<(FileType, u64, i64)> {
        log::trace!("get attr: {}", path);
        let ntfs_file = ntfs_file_from_path(&self.fs, &mut self.reader, path)?;
        let file_type = if ntfs_file.is_directory() {
            FileType::Directory
        } else {
            FileType::Regular
        };
        let size = ntfs_file_size(&ntfs_file, &mut self.reader)?;
        // Convert ntfs timestamp to unix timestamp (e.g. nano sec to sec and
        // substract ntfs "epoch" 01.01.1601 00:00:00
        let ts = ntfs_file.info()?.creation_time().nt_timestamp() as i64 / 10000000 - 11644473600;
        Ok((file_type, size, ts))
    }

    fn read_dir(&mut self, path: &str) -> Result<Vec<FileInfo>> {
        log::trace!("readdir {}", path);
        let ntfs_dir = ntfs_file_from_path(&self.fs, &mut self.reader, path)?;
        let mut ntfs_entries = Vec::new();

        let ntfs_index = ntfs_dir.directory_index(&mut self.reader)?;
        let mut index_entries = ntfs_index.entries();
        while let Some(entry) = index_entries.next(&mut self.reader) {
            let ntfs_file = entry?.to_file(&self.fs, &mut self.reader)?;
            // Filter MFT metafiles (https://en.wikipedia.org/wiki/NTFS#Metafiles)
            if ntfs_file.file_record_number() < 27 {
                continue;
            }

            let name_string = ntfs_name_from_file(&ntfs_file, &mut self.reader)?;
            if name_string == "." || name_string == ".." {
                continue;
            }
            // See get_attr() above
            let ts =
                ntfs_file.info()?.creation_time().nt_timestamp() as i64 / 10000000 - 11644473600;
            ntfs_entries.push(FileInfo {
                path: format!("{}/{}", path, name_string),
                size: ntfs_file_size(&ntfs_file, &mut self.reader)?,
                ftype: if ntfs_file.is_directory() {
                    FileType::Directory.into()
                } else {
                    FileType::Regular.into()
                },
                timestamp: ts,
            });
        }

        Ok(ntfs_entries)
    }

    fn read_file(
        &mut self,
        path: &str,
        buf: &mut Vec<u8>,
        offset: u64,
        bytes_to_read: u64,
    ) -> Result<u64> {
        let ntfs_file = ntfs_file_from_path(&self.fs, &mut self.reader, path)?;

        let data_item = match ntfs_file.data(&mut self.reader, "") {
            Some(data_item) => data_item?,
            None => {
                return Err(Error::FSError(format!(
                    "No ntfs data stream for file {}",
                    path
                )))
            }
        };
        let mut data_value = data_item.to_attribute().value(&mut self.reader)?;
        data_value.seek(&mut self.reader, SeekFrom::Start(offset))?;

        // read() while buffer isn't full or EOF is reached.
        // don't use read_exact() because it whould ret an error and fuse always asks 4kb
        let mut bytes_read = 0;
        loop {
            match data_value.read(&mut self.reader, &mut buf[bytes_read..]) {
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
        drop(self.fs);
        Ok(self.reader)
    }
}
