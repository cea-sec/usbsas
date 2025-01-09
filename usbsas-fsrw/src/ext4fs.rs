use crate::FSRead;
use crate::{Error, Result};
use positioned_io2::ReadAt;
use std::io::{Read, Seek, SeekFrom};
use usbsas_proto::common::{FileInfo, FileType};

pub struct Ext4<T> {
    vol: ext4::SuperBlock<T>,
}

impl<T: ReadAt> FSRead<T> for Ext4<T> {
    fn new(reader: T, _sector_size: u32) -> Result<Self> {
        let options = ext4::Options {
            checksums: ext4::Checksums::Enabled,
        };
        let vol = ext4::SuperBlock::new_with_options(reader, &options)?;
        Ok(Ext4 { vol })
    }

    fn get_attr(&mut self, path: &str) -> Result<(FileType, u64, i64)> {
        let entry = self.vol.resolve_path(path)?;
        let inode = self.vol.load_inode(entry.inode)?;
        let file_type = match entry.file_type {
            ext4::FileType::RegularFile => FileType::Regular,
            ext4::FileType::Directory => FileType::Directory,
            _ => FileType::Other,
        };
        Ok((file_type, inode.stat.size, inode.stat.ctime.epoch_secs))
    }

    fn read_dir(&mut self, path: &str) -> Result<Vec<FileInfo>> {
        let entry = self.vol.resolve_path(path)?;
        let inode = self.vol.load_inode(entry.inode)?;
        let mut files_info = vec![];
        let enhanced = self.vol.enhance(&inode)?;
        match enhanced {
            ext4::Enhanced::Directory(entries) => {
                for entry in entries {
                    if &entry.name == "." || &entry.name == ".." || &entry.name == "lost+found" {
                        continue;
                    }
                    let inode = self.vol.load_inode(entry.inode)?;
                    let file_type = match entry.file_type {
                        ext4::FileType::RegularFile => FileType::Regular,
                        ext4::FileType::Directory => FileType::Directory,
                        _ => FileType::Other,
                    };
                    files_info.push(FileInfo {
                        path: format!("{}/{}", path.trim_end_matches('/'), &entry.name),
                        ftype: file_type.into(),
                        size: inode.stat.size,
                        timestamp: inode.stat.ctime.epoch_secs,
                    });
                }
            }
            _ => {
                return Err(Error::FSError("Cannot list a non dir entry".into()));
            }
        }
        Ok(files_info)
    }

    fn read_file(
        &mut self,
        path: &str,
        buf: &mut Vec<u8>,
        offset: u64,
        bytes_to_read: u64,
    ) -> Result<u64> {
        let entry = self.vol.resolve_path(path)?;
        let inode = self.vol.load_inode(entry.inode)?;
        let mut reader = self.vol.open(&inode)?;
        reader.seek(SeekFrom::Start(offset))?;

        // read() while buffer isn't full or EOF is reached.
        // don't use read_exact() because it would ret an error and fuse always asks 4kb
        let mut bytes_read = 0;
        loop {
            match reader.read(&mut buf[bytes_read..]) {
                Ok(size) => {
                    bytes_read += size;
                    if bytes_read as u64 == bytes_to_read || size == 0 {
                        return Ok(bytes_read as u64);
                    }
                }
                Err(err) => {
                    log::error!("read error: {}", err);
                    return Err(Error::IO(err));
                }
            }
        }
    }

    fn unmount_fs(self: Box<Self>) -> Result<T> {
        Ok(self.vol.into_inner())
    }
}
