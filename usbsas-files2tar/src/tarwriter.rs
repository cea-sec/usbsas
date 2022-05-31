use crate::{ArchiveWriter, TAR_BLOCK_SIZE};
use crate::{Error, Result};
use serde_json::json;
use std::{io::Write, path::Path, time::SystemTime};
use usbsas_proto::common::FileType;

// If prefix is not None, all files will be stored in the archive under a /[prefix] directory.
// Some infos about the transfer will also be written in a /infos.json file.
pub(crate) struct TarWriter<W: Write> {
    builder: tar::Builder<W>,
    prefix: Option<String>,
    files: Vec<String>,
}

impl<W: Write> TarWriter<W> {
    pub(crate) fn new(writer: W, prefix: Option<String>) -> Self {
        TarWriter {
            builder: tar::Builder::new(writer),
            prefix,
            files: Vec::new(),
        }
    }
}

impl<W: Write> ArchiveWriter for TarWriter<W> {
    fn init(&mut self) -> Result<()> {
        self.builder.follow_symlinks(false);
        if let Some(prefix) = &self.prefix {
            let mut header = tar::Header::new_ustar();
            header.set_size(0);
            header.set_entry_type(tar::EntryType::Directory);
            header.set_mode(0o755);
            header.set_path(prefix)?;
            header.set_cksum();
            self.builder.append(&header, std::io::empty())?;
        }
        Ok(())
    }

    fn newfile(&mut self, path: &str, ftype: FileType, size: u64, timestamp: i64) -> Result<()> {
        let mut header = tar::Header::new_ustar();
        match ftype {
            FileType::Regular => {
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
        if let Some(prefix) = &self.prefix {
            path_string.insert_str(0, prefix);
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

    fn finish(mut self: Box<Self>, req: usbsas_proto::writetar::RequestClose) -> Result<()> {
        if self.prefix.is_some() {
            let mut name = match uname::Info::new() {
                Ok(uname) => uname.nodename,
                _ => "Unknown".to_string(),
            };
            name = format!("USBSAS-{}", name);
            let infos = json!({
                "time": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64(),
                "name": name,
                "id": req.id,
                "file_names": self.files,
                "vendorid": req.vendorid,
                "productid": req.productid,
                "manufacturer": req.manufacturer,
                "serial": req.serial,
                "description": req.description
            })
            .to_string();
            let mut header = tar::Header::new_ustar();
            header.set_size(infos.as_bytes().len() as u64);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_path("infos.json")?;
            header.set_mtime(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)?
                    .as_secs_f64() as u64,
            );
            header.set_cksum();
            self.builder.append(&header, infos.as_bytes())?;
        }

        // Make sure everything is flushed and closed
        let mut inner = self.builder.into_inner()?;
        inner.flush()?;
        drop(inner);
        Ok(())
    }
}
