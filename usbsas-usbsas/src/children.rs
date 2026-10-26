//! Child processes of usbsas.

use anyhow::Result;
use usbsas_comm::{
    ComRqAnalyzer, ComRqCmdExec, ComRqDownloader, ComRqFiles, ComRqFs2Dev, ComRqIdentifier,
    ComRqUploader, ComRqUsbDev, ComRqWriteDst, ProtoRespUsbsas,
};
use usbsas_process::{ChildMngt, UsbsasChild, UsbsasChildSpawner};

/// Every child process of a session, with the communication channel used to
/// send them requests.
pub struct Children {
    pub analyzer: UsbsasChild<ComRqAnalyzer>,
    pub identifier: UsbsasChild<ComRqIdentifier>,
    pub cmdexec: UsbsasChild<ComRqCmdExec>,
    pub downloader: UsbsasChild<ComRqDownloader>,
    pub files2fs: UsbsasChild<ComRqWriteDst>,
    /// Writes source files into the session tar
    pub files2tar: UsbsasChild<ComRqWriteDst>,
    /// Writes analyzed (clean) files into the session clean tar
    pub files2cleantar: UsbsasChild<ComRqWriteDst>,
    pub fs2dev: UsbsasChild<ComRqFs2Dev>,
    pub local2files: UsbsasChild<ComRqFiles>,
    pub scsi2files: UsbsasChild<ComRqFiles>,
    pub tar2files: UsbsasChild<ComRqFiles>,
    pub uploader: UsbsasChild<ComRqUploader>,
    pub usbdev: UsbsasChild<ComRqUsbDev>,
}

impl Children {
    /// Spawn all children. `tar_path` and `fs_path` are the session's working
    /// files.
    ///
    /// Children spawned with `wait_on_startup()` are locked: they must be
    /// unlocked with [`ChildMngt::unlock_with`] before being used, `0` meaning
    /// they won't be needed this session.
    pub fn spawn(config_path: &str, tar_path: &str, fs_path: &str) -> Result<Self> {
        log::trace!("spawn children");
        let identifier = UsbsasChildSpawner::new("usbsas-identifier").spawn::<ComRqIdentifier>()?;
        let cmdexec = UsbsasChildSpawner::new("usbsas-cmdexec")
            .arg(tar_path)
            .arg(fs_path)
            .args(&["-c", config_path])
            .wait_on_startup()
            .spawn::<ComRqCmdExec>()?;
        let downloader = UsbsasChildSpawner::new("usbsas-downloader")
            .arg(tar_path)
            .args(&["-c", config_path])
            .spawn::<ComRqDownloader>()?;
        let usbdev = UsbsasChildSpawner::new("usbsas-usbdev")
            .args(&["-c", config_path])
            .spawn::<ComRqUsbDev>()?;
        let scsi2files = UsbsasChildSpawner::new("usbsas-scsi2files").spawn::<ComRqFiles>()?;
        let local2files = UsbsasChildSpawner::new("usbsas-local2files")
            .args(&["-c", config_path])
            .spawn::<ComRqFiles>()?;
        let files2tar = UsbsasChildSpawner::new("usbsas-files2tar")
            .arg(tar_path)
            .spawn::<ComRqWriteDst>()?;
        let files2cleantar = UsbsasChildSpawner::new("usbsas-files2tar")
            .arg(&format!("{}_clean.tar", tar_path.trim_end_matches(".tar")))
            .spawn::<ComRqWriteDst>()?;
        let files2fs = UsbsasChildSpawner::new("usbsas-files2fs")
            .arg(fs_path)
            .spawn::<ComRqWriteDst>()?;
        let fs2dev = UsbsasChildSpawner::new("usbsas-fs2dev")
            .arg(fs_path)
            .wait_on_startup()
            .spawn::<ComRqFs2Dev>()?;
        let tar2files = UsbsasChildSpawner::new("usbsas-tar2files")
            .arg(tar_path)
            .wait_on_startup()
            .spawn::<ComRqFiles>()?;
        let uploader = UsbsasChildSpawner::new("usbsas-uploader")
            .arg(tar_path)
            .args(&["-c", config_path])
            .wait_on_startup()
            .spawn::<ComRqUploader>()?;
        let analyzer = UsbsasChildSpawner::new("usbsas-analyzer")
            .arg(tar_path)
            .args(&["-c", config_path])
            .spawn::<ComRqAnalyzer>()?;

        Ok(Self {
            analyzer,
            identifier,
            cmdexec,
            downloader,
            files2fs,
            files2tar,
            files2cleantar,
            fs2dev,
            local2files,
            scsi2files,
            tar2files,
            uploader,
            usbdev,
        })
    }

    /// All children, to apply an action on each of them.
    pub fn as_array_mut(&mut self) -> Vec<&mut dyn ChildMngt> {
        vec![
            &mut self.analyzer,
            &mut self.identifier,
            &mut self.cmdexec,
            &mut self.downloader,
            &mut self.files2fs,
            &mut self.files2tar,
            &mut self.files2cleantar,
            &mut self.fs2dev,
            &mut self.local2files,
            &mut self.scsi2files,
            &mut self.tar2files,
            &mut self.uploader,
            &mut self.usbdev,
        ]
    }

    /// End all children, wait for them to exit, then answer the client's `End`
    /// request.
    pub fn end_wait_all(&mut self, comm: &mut impl ProtoRespUsbsas) -> Result<()> {
        log::trace!("ending children");
        for child in self.as_array_mut().iter_mut() {
            child.end().ok();
        }
        log::trace!("waiting children");
        for child in self.as_array_mut().iter_mut() {
            child.wait().ok();
        }
        comm.end()?;
        Ok(())
    }
}
