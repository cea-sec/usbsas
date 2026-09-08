use crate::FSRead;
use crate::{Error, Result};
use ext4_view::{Ext4Read, FileType as Ext4FileType};
use std::cell::RefCell;
use std::io::{Read, Seek, SeekFrom};
use std::rc::Rc;
use usbsas_proto::common::{FileInfo, FileType};

struct SharedReader<T>(Rc<RefCell<Option<T>>>);

impl<T: Read + Seek> Ext4Read for SharedReader<T> {
    fn read(
        &mut self,
        start_byte: u64,
        dst: &mut [u8],
    ) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        let mut guard = self.0.borrow_mut();
        let reader = guard
            .as_mut()
            .ok_or_else(|| std::io::Error::other("ext4 reader error"))?;
        reader.seek(SeekFrom::Start(start_byte))?;
        reader.read_exact(dst)?;
        Ok(())
    }
}

fn ftype(ft: Ext4FileType) -> FileType {
    match ft {
        Ext4FileType::Regular => FileType::Regular,
        Ext4FileType::Directory => FileType::Directory,
        _ => FileType::Other,
    }
}

pub struct Ext4<T> {
    fs: ext4_view::Ext4,
    inner: Rc<RefCell<Option<T>>>,
}

impl<T: Read + Seek + 'static> FSRead<T> for Ext4<T> {
    fn new(reader: T, _sector_size: u32) -> Result<Self> {
        let inner = Rc::new(RefCell::new(Some(reader)));
        let fs = ext4_view::Ext4::load(Box::new(SharedReader(Rc::clone(&inner))))?;
        Ok(Ext4 { fs, inner })
    }

    fn get_attr(&mut self, path: &str) -> Result<(FileType, u64, i64)> {
        let md = self.fs.symlink_metadata(path)?;
        Ok((ftype(md.file_type()), md.len(), md.modified().seconds()))
    }

    fn read_dir(&mut self, path: &str) -> Result<Vec<FileInfo>> {
        let mut files_info = vec![];
        for entry in self.fs.read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == "." || name == ".." || name == "lost+found" {
                continue;
            }
            let name = match name.as_str() {
                Ok(name) => name,
                Err(_) => {
                    log::warn!("skipping non utf-8 filename: {}", name.display());
                    continue;
                }
            };
            let md = entry.metadata()?;
            files_info.push(FileInfo {
                path: format!("{}/{}", path.trim_end_matches('/'), name),
                ftype: ftype(md.file_type()).into(),
                size: md.len(),
                timestamp: md.modified().seconds(),
            });
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
        let mut file = self.fs.open(path)?;
        file.seek_to(offset)?;

        // read() while buffer isn't full or EOF is reached.
        // don't use read_exact() because it would ret an error and fuse always asks 4kb
        let mut bytes_read = 0;
        loop {
            match file.read_bytes(&mut buf[bytes_read..]) {
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
        let Ext4 { fs, inner } = *self;
        drop(fs);
        inner
            .borrow_mut()
            .take()
            .ok_or_else(|| Error::FSError("ext4 reader was taken".into()))
    }
}
