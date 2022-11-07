use crate::error::ServiceError;
use std::{fs, path::Path};

pub(crate) struct TmpFiles {
    pub(crate) out_tar: String,
    pub(crate) out_fs: String,
    pub(crate) out_directory: String,
}

impl TmpFiles {
    pub(crate) fn new(out_directory: String) -> Result<Self, ServiceError> {
        let (out_tar, out_fs) = TmpFiles::create_files(&out_directory)?;
        Ok(TmpFiles {
            out_tar,
            out_fs,
            out_directory,
        })
    }

    fn create_files(out_directory: &str) -> Result<(String, String), ServiceError> {
        let (_, out_tar) = tempfile::Builder::new()
            .prefix("usbsas_out_")
            .suffix(".tar")
            .rand_bytes(6)
            .tempfile_in(out_directory)?
            .keep()?;
        let out_tar = out_tar.as_path().display().to_string();

        let (_, out_fs) = tempfile::Builder::new()
            .prefix("usbsas_fs_")
            .suffix(".bin")
            .rand_bytes(6)
            .tempfile_in(out_directory)?
            .keep()?;
        let out_fs = out_fs.as_path().display().to_string();
        Ok((out_tar, out_fs))
    }

    fn delete_files(&self) {
        // XXX TODO add a config to always remove or not ?
        if let Ok(metadata) = fs::metadata(&self.out_fs) {
            if metadata.len() == 0 {
                let _ = fs::remove_file(Path::new(&self.out_fs)).ok();
            }
        };

        if let Ok(metadata) = fs::metadata(&self.out_tar) {
            if metadata.len() == 0 {
                let _ = fs::remove_file(Path::new(&self.out_tar)).ok();
            }
        };
    }

    pub(crate) fn reset(&mut self) -> Result<(), ServiceError> {
        self.delete_files();
        let (new_out_tar, new_out_fs) = TmpFiles::create_files(&self.out_directory)?;
        self.out_tar = new_out_tar;
        self.out_fs = new_out_fs;
        Ok(())
    }
}

impl Drop for TmpFiles {
    fn drop(&mut self) {
        self.delete_files()
    }
}
