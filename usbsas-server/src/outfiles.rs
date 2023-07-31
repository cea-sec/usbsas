use crate::error::ServiceError;
use std::{
    io::{self, ErrorKind},
    {
        fs::{self, File},
        path::Path,
    },
};

pub(crate) struct OutFiles {
    pub(crate) out_tar: String,
    pub(crate) out_fs: String,
    pub(crate) out_directory: String,
}

impl OutFiles {
    pub(crate) fn new(out_directory: String, session_id: &str) -> Result<Self, ServiceError> {
        let (out_tar, out_fs) = OutFiles::create_files(&out_directory, session_id)?;
        Ok(OutFiles {
            out_tar,
            out_fs,
            out_directory,
        })
    }

    fn create_files(
        out_directory: &str,
        session_id: &str,
    ) -> Result<(String, String), ServiceError> {
        let out_dir_path = Path::new(&out_directory);
        if !out_dir_path.is_dir() {
            return Err(io::Error::new(
                ErrorKind::NotFound,
                format!("{} does not exist or isn't a directory", out_directory),
            )
            .into());
        }

        let out_tar_path = out_dir_path.join(format!("usbsas_{}.tar", session_id));
        let out_fs_path = out_dir_path.join(format!("usbsas_{}.img", session_id));

        let _ = File::create(&out_tar_path)?;
        let _ = File::create(&out_fs_path)?;

        Ok((
            out_tar_path.to_string_lossy().to_string(),
            out_fs_path.to_string_lossy().to_string(),
        ))
    }

    fn delete_if_empty(&self) {
        if let Ok(metadata) = fs::metadata(&self.out_fs) {
            if metadata.len() == 0 {
                let _ = fs::remove_file(Path::new(&self.out_fs)).ok();
            }
        };

        if let Ok(metadata) = fs::metadata(&self.out_tar) {
            // 1536 == tar with only a data entry (512b) + 1024b zeroes
            if metadata.len() == 1536 {
                let _ = fs::remove_file(Path::new(&self.out_tar)).ok();
            }
        };
    }

    pub(crate) fn reset(&mut self, session_id: &str) -> Result<(), ServiceError> {
        self.delete_if_empty();
        let (new_out_tar, new_out_fs) = OutFiles::create_files(&self.out_directory, session_id)?;
        self.out_tar = new_out_tar;
        self.out_fs = new_out_fs;
        Ok(())
    }
}

impl Drop for OutFiles {
    fn drop(&mut self) {
        self.delete_if_empty()
    }
}
