use crate::ArchiveWriter;
use crate::{Error, Result};
use std::{io::Write, path::Path};
use usbsas_proto::common::FileType;
use usbsas_utils::{TAR_BLOCK_SIZE, TAR_DATA_DIR};

pub(crate) struct TarWriter<W: Write> {
    builder: tar::Builder<W>,
    data_dir: String,
    files: Vec<String>,
}

impl<W: Write> TarWriter<W> {
    pub(crate) fn new(writer: W) -> Self {
        TarWriter {
            builder: tar::Builder::new(writer),
            data_dir: TAR_DATA_DIR.trim_end_matches('/').to_owned() + "/",
            files: Vec::new(),
        }
    }
}

impl<W: Write> ArchiveWriter for TarWriter<W> {
    fn init(&mut self) -> Result<()> {
        self.builder.follow_symlinks(false);
        let mut header = tar::Header::new_ustar();
        header.set_size(0);
        header.set_entry_type(tar::EntryType::Directory);
        header.set_mode(0o755);
        header.set_path(self.data_dir.clone())?;
        header.set_cksum();
        self.builder.append(&header, std::io::empty())?;
        Ok(())
    }

    fn newfile(&mut self, path: &str, ftype: FileType, size: u64, timestamp: i64) -> Result<()> {
        let mut header = tar::Header::new_ustar();
        match ftype {
            FileType::Regular | FileType::Metadata => {
                header.set_size(size);
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(0o644);
            }
            FileType::Directory => {
                header.set_size(0);
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(0o755);
            }
            _ => return Err(Error::Error("Bad file type".to_string())),
        }
        header.set_mtime(timestamp as u64);
        let mut path_string: String = path.trim_start_matches('/').into();
        self.files.push(path_string.clone());
        if !matches!(ftype, FileType::Metadata) {
            path_string.insert_str(0, &self.data_dir);
        }
        self.builder
            .append_data(&mut header, Path::new(&path_string), std::io::empty())?;
        Ok(())
    }

    fn writefile(&mut self, data: &[u8]) -> Result<()> {
        self.builder.get_mut().write_all(data)?;
        Ok(())
    }

    fn endfile(&mut self, len_written: usize) -> Result<()> {
        // Pad to size of block
        let buf = [0; TAR_BLOCK_SIZE];
        let remaining = TAR_BLOCK_SIZE - (len_written % TAR_BLOCK_SIZE);
        if remaining < TAR_BLOCK_SIZE {
            self.builder.get_mut().write_all(&buf[..remaining])?;
        }
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<()> {
        // Make sure everything is flushed and closed
        let mut inner = self.builder.into_inner()?;
        inner.flush()?;
        drop(inner);
        Ok(())
    }
}
