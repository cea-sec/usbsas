use crate::{Error, Result};
use crate::{FSRead, FSWrite, WriteSeek};
use ntfs::NtfsReadSeek;
use std::{
    collections::{BTreeMap, HashMap},
    convert::TryFrom,
    io::{Read, Seek, SeekFrom, Write},
};
use usbsas_proto::common::{FileInfo, FileType, FsType};

pub struct NTFS3G<T> {
    volume: ntfs3g::Ntfs3g<T>,
}

impl<T: Read + Write + Seek> FSWrite<T> for NTFS3G<T> {
    fn mkfs(writer: T, sector_size: u64, sector_count: u64, _fstype: Option<FsType>) -> Result<Self>
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
        log::trace!("new file {path}");
        let file: Box<dyn WriteSeek> = Box::new(
            self.volume
                .new_file(path, timestamp)
                .map_err(|err| Error::FSError(format!("Couldn't create file {path}: {err}")))?,
        );
        Ok(file)
    }

    fn newdir(&mut self, path: &str, timestamp: i64) -> Result<()> {
        log::trace!("new dir {path}");
        self.volume
            .new_dir(path, timestamp)
            .map_err(|err| Error::FSError(format!("Couldn't create dir {path}: {err}")))
    }

    fn removefile(&mut self, path: &str) -> Result<()> {
        log::trace!("remove file {path}");
        self.volume
            .remove_file(path)
            .map_err(|err| Error::FSError(format!("Couldn't remove file {path}: {err}")))
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
    file_cache: HashMap<String, u64>,
}

fn ntfs_file_from_path<'a, T: Read + Seek>(
    fs: &'a ntfs::Ntfs,
    reader: &mut T,
    path: &str,
    file_cache: &mut HashMap<String, u64>,
) -> Result<ntfs::NtfsFile<'a>> {
    if let Some(file_record_number) = file_cache.get(path) {
        return Ok(fs.file(reader, *file_record_number)?);
    }
    let split_path = path.trim_end_matches('/').split('/');
    let mut cur_file = fs.root_directory(reader)?;
    let mut absolute_cur_path = String::from("");

    for cur_path in split_path {
        if cur_path.is_empty() {
            // Already opened root dir if path starts with a '/'
            continue;
        }
        let absolute_cur_path_new = format!("{absolute_cur_path}/{cur_path}");
        if let Some(file_record_number) = file_cache.get(&absolute_cur_path_new) {
            // We already walked this parent dir or cached the file
            absolute_cur_path = absolute_cur_path_new;
            cur_file = fs.file(reader, *file_record_number)?;
            continue;
        }
        // Cache current index
        let mut found = false;
        let index = cur_file.directory_index(reader)?;
        let mut index_entries = index.entries();
        while let Some(entry) = index_entries.next(reader) {
            let entry = entry?.to_file(fs, reader)?;
            let name_string = ntfs_name_from_file(&entry, reader)?;
            if name_string == cur_path {
                found = true;
            }
            file_cache.insert(
                format!("{absolute_cur_path}/{name_string}"),
                entry.file_record_number(),
            );
        }
        if !found {
            return Err(Error::FSError(format!("didn't find file '{path}'")));
        } else {
            absolute_cur_path = absolute_cur_path_new;
        }
    }

    if let Some(file_record_number) = file_cache.get(path) {
        Ok(fs.file(reader, *file_record_number)?)
    } else {
        Err(Error::FSError(format!("didn't find file '{path}'")))
    }
}

fn ntfs_name_from_file<T: Read + Seek>(file: &ntfs::NtfsFile, reader: &mut T) -> Result<String> {
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

    Ok(name?.name().to_string_lossy())
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
        Ok(data_item.to_attribute()?.value(reader)?.len())
    }
}

impl<T: Read + Seek> FSRead<T> for NTFS<T> {
    fn new(mut reader: T, _sector_size: u32) -> Result<Self> {
        let fs = ntfs::Ntfs::new(&mut reader)?;
        let mut file_cache = HashMap::new();
        let root_dir = fs.root_directory(&mut reader)?;
        file_cache.insert("".to_owned(), root_dir.file_record_number());
        file_cache.insert("/".to_owned(), root_dir.file_record_number());
        Ok(NTFS {
            reader,
            fs,
            file_cache,
        })
    }

    fn get_attr(&mut self, path: &str) -> Result<(FileType, u64, i64)> {
        log::trace!("get attr: {path}");
        let ntfs_file =
            ntfs_file_from_path(&self.fs, &mut self.reader, path, &mut self.file_cache)?;
        let file_type = if ntfs_file.is_directory() {
            FileType::Directory
        } else {
            FileType::Regular
        };
        let size = ntfs_file_size(&ntfs_file, &mut self.reader)?;
        // Convert ntfs timestamp to unix timestamp (e.g. nano sec to sec and
        // subtract ntfs "epoch" 01.01.1601 00:00:00
        let ts = ntfs_file.info()?.creation_time().nt_timestamp() as i64 / 10000000 - 11644473600;
        Ok((file_type, size, ts))
    }

    fn read_dir(&mut self, path: &str) -> Result<Vec<FileInfo>> {
        log::trace!("readdir {path}");
        let ntfs_dir = ntfs_file_from_path(&self.fs, &mut self.reader, path, &mut self.file_cache)?;
        let mut ntfs_entries: BTreeMap<u64, FileInfo> = BTreeMap::new();

        let ntfs_index = ntfs_dir.directory_index(&mut self.reader)?;
        let mut index_entries = ntfs_index.entries();
        while let Some(entry) = index_entries.next(&mut self.reader) {
            let ntfs_file = entry?.to_file(&self.fs, &mut self.reader)?;
            if ntfs_entries.contains_key(&ntfs_file.file_record_number()) {
                continue;
            }

            let name_string = ntfs_name_from_file(&ntfs_file, &mut self.reader)?;
            // Filter MFT metafiles (https://en.wikipedia.org/wiki/NTFS#Metafiles)
            if ntfs_file.file_record_number() < 27 && name_string.starts_with('$') {
                continue;
            }

            if name_string == "." || name_string == ".." {
                continue;
            }
            // See get_attr() above
            let ts =
                ntfs_file.info()?.creation_time().nt_timestamp() as i64 / 10000000 - 11644473600;
            ntfs_entries.insert(
                ntfs_file.file_record_number(),
                FileInfo {
                    path: format!("{}/{name_string}", path.trim_end_matches('/')),
                    size: ntfs_file_size(&ntfs_file, &mut self.reader)?,
                    ftype: if ntfs_file.is_directory() {
                        FileType::Directory.into()
                    } else {
                        FileType::Regular.into()
                    },
                    timestamp: ts,
                },
            );
        }

        Ok(Vec::from_iter(ntfs_entries.into_values()))
    }

    fn read_file(
        &mut self,
        path: &str,
        buf: &mut Vec<u8>,
        offset: u64,
        bytes_to_read: u64,
    ) -> Result<u64> {
        let ntfs_file =
            ntfs_file_from_path(&self.fs, &mut self.reader, path, &mut self.file_cache)?;

        let data_item = match ntfs_file.data(&mut self.reader, "") {
            Some(data_item) => data_item?,
            None => {
                return Err(Error::FSError(format!(
                    "No ntfs data stream for file {path}"
                )))
            }
        };
        let mut data_value = data_item.to_attribute()?.value(&mut self.reader)?;
        data_value.seek(&mut self.reader, SeekFrom::Start(offset))?;

        // read() while buffer isn't full or EOF is reached.
        // don't use read_exact() because it would ret an error and fuse always asks 4kb
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
                    log::error!("read error: {err}");
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
