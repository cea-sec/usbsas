use crate::ArchiveWriter;
use crate::{Error, Result};
use std::{
    io::Write,
    path::{Component, Path},
};
use usbsas_proto::common::FileType;
use usbsas_utils::{TAR_BLOCK_SIZE, TAR_DATA_DIR};

/// Tar (ustar) [`ArchiveWriter`].
pub(crate) struct TarWriter<W: Write> {
    builder: tar::Builder<W>,
    data_dir: String,
}

impl<W: Write> TarWriter<W> {
    pub(crate) fn new(writer: W) -> Self {
        TarWriter {
            builder: tar::Builder::new(writer),
            data_dir: TAR_DATA_DIR.trim_end_matches('/').to_owned() + "/",
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

    /// Files are stored under the data directory, except metadata files.
    /// Paths containing `..` are refused, and files need a name (but the root
    /// directory `/` is accepted, written as the data directory).
    fn newfile(&mut self, path: &str, ftype: FileType, size: u64, timestamp: i64) -> Result<()> {
        let mut path_string: String = path.trim_start_matches('/').into();
        let mut components = Path::new(&path_string).components();
        if components.clone().any(|c| c == Component::ParentDir) {
            return Err(Error::Error("Bad file name".to_string()));
        }
        if ftype != FileType::Directory && !components.any(|c| matches!(c, Component::Normal(_))) {
            return Err(Error::Error("Bad file name".to_string()));
        }
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

    /// Pad the file data to a tar block.
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
