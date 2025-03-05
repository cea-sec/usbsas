use crate::{
    filter::{Rule, Rules},
    Children, Devices, Transfer, TransferFiles, TransferReport,
};
use anyhow::{anyhow, bail, Context, Result};
use log::{debug, error, info, trace};
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use usbsas_comm::{
    ComRqFiles, ComRqWriteDst, ProtoReqAnalyzer, ProtoReqCmdExec, ProtoReqCommon,
    ProtoReqDownloader, ProtoReqFiles, ProtoReqFs2Dev, ProtoReqIdentificator, ProtoReqUploader,
    ProtoReqUsbDev, ProtoReqWriteDst, ProtoRespUsbsas,
};
use usbsas_config::Config;
use usbsas_process::{ChildMngt, UsbsasChild};
use usbsas_proto::{
    self as proto,
    common::{device::Device, FileType, FsType, Network, OutFileType, Status, UsbDevice},
    usbsas::request::Msg,
};
use usbsas_utils::READ_FILE_MAX_SIZE;

pub enum State {
    Init(InitState),
    OpenSrcUsb(OpenSrcUsbState),
    BrowseSrc(BrowseSrcState),
    DownloadSrc(DownloadSrcState),
    FileSelection(FileSelectionState),
    Analyze(AnalyzeState),
    WriteDstFile(WriteDstFileState),
    TransferDst(TransferDstState),
    ImgDisk(ImgDiskState),
    Wipe(WipeState),
    End(EndState),
    Exit,
}

impl State {
    pub fn run(self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm, children),
            State::OpenSrcUsb(s) => s.run(comm, children),
            State::BrowseSrc(s) => s.run(comm, children),
            State::DownloadSrc(s) => s.run(comm, children),
            State::FileSelection(s) => s.run(comm, children),
            State::Analyze(s) => s.run(comm, children),
            State::WriteDstFile(s) => s.run(comm, children),
            State::TransferDst(s) => s.run(comm, children),
            State::Wipe(s) => s.run(comm, children),
            State::ImgDisk(s) => s.run(comm, children),
            State::End(s) => s.run(comm, children),
            State::Exit => std::process::exit(0),
        }
    }
}

// Shared functions for states
pub trait RunState {
    fn run(self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<State>;

    fn devices(
        &mut self,
        comm: &mut impl ProtoRespUsbsas,
        children: &mut Children,
        include_alt: bool,
        devices: &mut Devices,
    ) -> Result<()> {
        trace!("handle req devices");
        // refresh plugged usb devices
        let mut usb_devices = children
            .usbdev
            .comm
            .devices(proto::usbdev::RequestDevices {})?
            .devices;
        devices.retain(|_, dev| !matches!(dev, Device::Usb(_)));
        while let Some(dev) = usb_devices.pop() {
            let device = Device::Usb(dev);
            devices.insert(device.id(), device);
        }
        comm.devices(proto::usbsas::ResponseDevices {
            devices: devices
                .iter_mut()
                .filter(|(_, dev)| match dev {
                    Device::Usb(_) => true,
                    _ => include_alt,
                })
                .map(|(_, dev)| dev.clone().into())
                .collect(),
        })?;
        Ok(())
    }

    fn userid(
        &self,
        comm: &mut impl ProtoRespUsbsas,
        children: &mut Children,
    ) -> Result<Option<String>> {
        trace!("handle req ID");
        let userid = children
            .identificator
            .comm
            .userid(proto::identificator::RequestUserId {})?
            .userid;
        if userid.is_empty() {
            comm.error("empty ID")?;
            return Ok(None);
        }
        comm.userid(proto::usbsas::ResponseUserId {
            userid: userid.clone(),
        })?;
        Ok(Some(userid))
    }

    fn forward_bitvec(&self, children: &mut Children) -> Result<()>
    where
        Self: Sized,
    {
        loop {
            let rep = children
                .files2fs
                .comm
                .bitvec(proto::writedst::RequestBitVec {})?;
            children
                .fs2dev
                .comm
                .loadbitvec(proto::fs2dev::RequestLoadBitVec {
                    chunk: rep.chunk,
                    last: rep.last,
                })?;
            if rep.last {
                break;
            }
        }
        Ok(())
    }

    fn write_report(
        &self,
        writer: &mut UsbsasChild<ComRqWriteDst>,
        report: &TransferReport,
        path: String,
    ) -> Result<()> {
        let report_data = serde_json::to_vec_pretty(&json!(report))?;
        writer.comm.newfile(proto::writedst::RequestNewFile {
            path: path.clone(),
            size: report_data.len() as u64,
            ftype: FileType::Metadata.into(),
            timestamp: report.timestamp,
        })?;
        writer.comm.writefile(proto::writedst::RequestWriteFile {
            path: path.clone(),
            offset: 0,
            data: report_data,
        })?;
        writer
            .comm
            .endfile(proto::writedst::RequestEndFile { path })?;
        Ok(())
    }
}

pub struct InitState {
    pub config: Config,
    pub plugged_devices: Vec<UsbDevice>,
}

impl RunState for InitState {
    fn run(mut self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<State> {
        let mut userid: Option<String> = None;
        let mut devices: Devices = HashMap::new();

        // Get alt devices from config
        self.config.networks.as_ref().inspect(|networks| {
            networks.iter().for_each(|network| {
                let mut net = Network::from(network);
                net.is_dst = true;
                let device = Device::Network(net);
                devices.insert(device.id(), device);
            })
        });
        self.config.source_network.as_ref().inspect(|network| {
            let mut net = proto::common::Network::from(*network);
            net.is_src = true;
            let device = Device::Network(net);
            devices.insert(device.id(), device);
        });
        self.config.command.as_ref().inspect(|command| {
            let device = Device::Command(proto::common::Command::from(*command));
            devices.insert(device.id(), device);
        });

        loop {
            match comm.recv_req()? {
                Msg::UserId(_) => {
                    userid = self.userid(comm, children)?;
                }
                Msg::Devices(req) => self
                    .devices(comm, children, req.include_alt, &mut devices)
                    .context("listing devices")?,
                Msg::InitTransfer(req) => {
                    let outfstype = match self.init_checks(&req, userid.clone(), &devices) {
                        Ok(outfstype) => outfstype,
                        Err(err) => {
                            comm.error(err)?;
                            continue;
                        }
                    };
                    return self.init_transfer(
                        comm,
                        children,
                        req,
                        userid.clone(),
                        outfstype,
                        &mut devices,
                    );
                }
                Msg::ImgDisk(req) => match devices.remove(&req.id) {
                    Some(Device::Usb(dev)) => {
                        return Ok(State::ImgDisk(ImgDiskState { device: dev }))
                    }
                    _ => {
                        comm.error("no matching device for imaging")?;
                        continue;
                    }
                },
                Msg::Wipe(req) => match devices.remove(&req.id) {
                    Some(Device::Usb(dev)) => {
                        return Ok(State::Wipe(WipeState {
                            device: dev,
                            quick: req.quick,
                            outfstype: FsType::try_from(req.fstype).unwrap(),
                        }))
                    }
                    _ => {
                        bail!("no matching device for wiping, waiting end");
                    }
                },
                Msg::End(_) => {
                    children.end_wait_all(comm)?;
                    break;
                }
                _ => {
                    error!("bad req");
                    comm.error("bad request")?;
                    continue;
                }
            }
        }
        Ok(State::Exit)
    }
}

impl InitState {
    fn init_checks(
        &self,
        req: &proto::usbsas::RequestInitTransfer,
        userid: Option<String>,
        devices: &Devices,
    ) -> Result<Option<FsType>> {
        if let (Some(src), Some(dst)) = (devices.get(&req.source), devices.get(&req.destination)) {
            if userid.is_none() {
                bail!("Unidentified");
            }
            if !src.is_src() {
                bail!("Selected source device error");
            }
            if !dst.is_dst() {
                bail!("Selected destination device error")
            }
            if matches!(src, Device::Network(_)) && req.pin.is_none() {
                bail!("Transfer from network requested without pin");
            }
            if matches!(src, Device::Network(_)) && matches!(dst, Device::Network(_)) {
                bail!("Network to network transfer not supported")
            }
            let outfstype = if matches!(dst, Device::Usb(_)) {
                match req.fstype {
                    Some(fstype) => Some(FsType::try_from(fstype)?),
                    None => {
                        bail!("USB dest requested but no fstype specified");
                    }
                }
            } else {
                None
            };
            Ok(outfstype)
        } else {
            bail!("No matching device(s) for transfer");
        }
    }

    fn init_transfer(
        self,
        comm: &mut impl ProtoRespUsbsas,
        children: &mut Children,
        req: proto::usbsas::RequestInitTransfer,
        userid: Option<String>,
        outfstype: Option<FsType>,
        devices: &mut Devices,
    ) -> Result<State> {
        if let (Some(src), Some(mut dst)) = (
            devices.remove(&req.source),
            devices.remove(&req.destination),
        ) {
            // Get max size of destination
            let max_dst_size = if let Device::Usb(ref mut usbdev) = dst {
                children
                    .fs2dev
                    .unlock_with((u64::from(usbdev.devnum) << 32) | u64::from(usbdev.busnum))?;
                let dev_size = children
                    .fs2dev
                    .comm
                    .devsize(proto::fs2dev::RequestDevSize {})?
                    .size;
                usbdev.dev_size = Some(dev_size);
                // XXX be more precise, arbitrary 95% to keep space for filesystem
                Some(dev_size * 95 / 100)
            } else {
                // won't be needed, unlock with 0 (exit)
                children.fs2dev.unlock_with(0)?;
                None
            };

            let analyze = if let Some(ref conf) = self.config.analyzer {
                if matches!(src, Device::Usb(_)) {
                    match dst {
                        Device::Usb(_) => conf.analyze_usb,
                        Device::Network(_) => conf.analyze_net,
                        Device::Command(_) => conf.analyze_cmd,
                    }
                } else {
                    false
                }
            } else {
                false
            };

            let transfer = Transfer {
                src,
                dst,
                userid: userid.ok_or(anyhow!("Unidentified"))?,
                outfstype,
                max_dst_size,
                selected_size: None,
                analyze,
                files: TransferFiles::new(),
                analyze_report: None,
            };

            let state = match transfer.src {
                Device::Network(_) => State::DownloadSrc(DownloadSrcState {
                    config: self.config,
                    transfer,
                    pin: req.pin.unwrap(),
                }),
                Device::Usb(_) => State::OpenSrcUsb(OpenSrcUsbState {
                    config: self.config,
                    transfer,
                }),
                _ => {
                    bail!("Source device unsupported");
                }
            };

            comm.inittransfer(proto::usbsas::ResponseInitTransfer {})?;
            Ok(state)
        } else {
            bail!("no matching device(s) for transfer");
        }
    }
}

pub struct OpenSrcUsbState {
    config: Config,
    transfer: Transfer,
}

impl RunState for OpenSrcUsbState {
    fn run(mut self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<State> {
        let src_usb = match self.transfer.src {
            Device::Usb(ref mut usb) => usb,
            _ => bail!("Source is not a USB device"),
        };
        let device = children
            .scsi2files
            .comm
            .opendevice(proto::files::RequestOpenDevice {
                busnum: src_usb.busnum,
                devnum: src_usb.devnum,
            })?;
        src_usb.dev_size = Some(device.dev_size);
        src_usb.block_size = Some(device.block_size);
        // List partitions
        loop {
            match comm.recv_req()? {
                Msg::Partitions(_) => {
                    debug!("handle req partitions");
                    comm.partitions(proto::usbsas::ResponsePartitions {
                        partitions: children
                            .scsi2files
                            .comm
                            .partitions(proto::files::RequestPartitions {})?
                            .partitions,
                    })?;
                }
                Msg::OpenPartition(req) => {
                    children
                        .scsi2files
                        .comm
                        .openpartition(proto::files::RequestOpenPartition { index: req.index })?;
                    comm.openpartition(proto::usbsas::ResponseOpenPartition {})?;
                    break;
                }
                Msg::End(_) => {
                    children.end_wait_all(comm)?;
                    return Ok(State::Exit);
                }
                _ => {
                    comm.error("Unexpected request")?;
                    continue;
                }
            };
        }
        Ok(State::BrowseSrc(BrowseSrcState {
            config: self.config,
            transfer: self.transfer,
        }))
    }
}

pub struct BrowseSrcState {
    config: Config,
    transfer: Transfer,
}

impl RunState for BrowseSrcState {
    fn run(self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<State> {
        let src_reader: &mut UsbsasChild<ComRqFiles> = match self.transfer.src {
            Device::Usb(_) => &mut children.scsi2files,
            _ => unimplemented!(),
        };
        let selected = loop {
            match comm.recv_req()? {
                Msg::ReadDir(req) => {
                    match src_reader
                        .comm
                        .readdir(proto::files::RequestReadDir { path: req.path })
                    {
                        Ok(res) => comm.readdir(proto::usbsas::ResponseReadDir {
                            filesinfo: res.filesinfo,
                        })?,
                        Err(err) => comm.error(err)?,
                    }
                    continue;
                }
                Msg::GetAttr(req) => {
                    match src_reader
                        .comm
                        .getattr(proto::files::RequestGetAttr { path: req.path })
                    {
                        Ok(attrs) => comm.getattr(proto::usbsas::ResponseGetAttr {
                            ftype: attrs.ftype,
                            size: attrs.size,
                            timestamp: attrs.timestamp,
                        })?,
                        Err(err) => {
                            comm.error(err)?;
                        }
                    }
                    continue;
                }
                Msg::SelectFiles(req) => break req.selected,
                Msg::End(_) => {
                    children.end_wait_all(comm)?;
                    return Ok(State::Exit);
                }
                _ => {
                    comm.error("Unexpected request")?;
                    continue;
                }
            }
        };
        Ok(State::FileSelection(FileSelectionState {
            config: self.config,
            transfer: self.transfer,
            selected: VecDeque::from(selected),
        }))
    }
}

pub struct DownloadSrcState {
    config: Config,
    transfer: Transfer,
    pin: String,
}

impl RunState for DownloadSrcState {
    fn run(self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<State> {
        let remote_path = format!("{}/{}", &self.transfer.userid, self.pin);
        let archive_size = children
            .downloader
            .comm
            .archiveinfos(proto::downloader::RequestArchiveInfos { path: remote_path })?
            .size;
        if self
            .transfer
            .max_dst_size
            .is_some_and(|size| archive_size > size)
        {
            bail!("Files to download would be larger than destination size, aborting transfer");
        }
        children
            .downloader
            .comm
            .download(proto::downloader::RequestDownload {})?;
        loop {
            let status = children.downloader.comm.recv_status()?;
            comm.status(
                status.current,
                status.total,
                status.done,
                status.status.try_into()?,
            )?;
            if status.done {
                break;
            }
        }
        let selected = VecDeque::from(vec![String::from("/")]);
        Ok(State::FileSelection(FileSelectionState {
            config: self.config,
            transfer: self.transfer,
            selected,
        }))
    }
}

pub struct FileSelectionState {
    config: Config,
    transfer: Transfer,
    selected: VecDeque<String>,
}

impl RunState for FileSelectionState {
    fn run(mut self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<State> {
        let max_file_size = if matches!(self.transfer.outfstype, Some(FsType::Fat)) {
            Some(0xFFFF_FFFF)
        } else {
            None
        };
        let selected_size = self.selected_to_filtered_files(children, max_file_size)?;
        if self
            .transfer
            .max_dst_size
            .is_some_and(|size| selected_size > size)
        {
            bail!("Selected files size is larger than destination size, aborting transfer");
        }
        self.transfer.selected_size = Some(selected_size);

        if !matches!(self.transfer.src, Device::Network(_)) {
            comm.selectfiles(proto::usbsas::ResponseSelectFiles { selected_size })?;
            self.tar_src_files(comm, children, selected_size)?;
        }

        if self.transfer.files.files.is_empty() {
            comm.error("No files to copy")?;
            let report = self.transfer.to_report("Aborted, nothing to copy");
            return Ok(State::End(EndState {
                report: Some(report),
            }));
        }

        if self.transfer.analyze {
            children.cmdexec.unlock_with(2)?;
            children.uploader.unlock_with(2)?;
            Ok(State::Analyze(AnalyzeState {
                config: self.config,
                transfer: self.transfer,
            }))
        } else {
            children.cmdexec.unlock_with(1)?;
            children.uploader.unlock_with(1)?;
            Ok(State::WriteDstFile(WriteDstFileState {
                config: self.config,
                transfer: self.transfer,
            }))
        }
    }
}

impl FileSelectionState {
    /// Expand tree of selected files and directories and compute total file size
    /// Also apply filter rules from configuration file
    fn selected_to_filtered_files(
        &mut self,
        children: &mut Children,
        max_file_size: Option<u64>,
    ) -> Result<u64> {
        let src_reader = match self.transfer.src {
            Device::Usb(_) => &mut children.scsi2files,
            Device::Network(_) => {
                children.tar2files.unlock_with(1)?;
                &mut children.tar2files
            }
            _ => unimplemented!(),
        };
        let mut files_size = 0;
        // Read filter rules from config
        let mut all_entries = HashSet::new();
        let rules = Rules {
            rules: self
                .config
                .filters
                .take()
                .unwrap_or_default()
                .into_iter()
                .map(|f| Rule {
                    contain: f.contain,
                    start: f.start,
                    end: f.end,
                })
                .collect(),
        }
        .into_lowercase();
        while let Some(entry) = self.selected.pop_front() {
            if (matches!(self.transfer.src, Device::Network(_)) && entry == "config.json")
                || all_entries.contains(&entry)
            {
                continue;
            }
            // First add parent(s) of a file if not selected
            let mut parts = entry.trim_start_matches('/').split('/');
            // Remove last (file basename)
            let _ = parts.next_back();
            let mut parent = String::from("");
            for dir in parts {
                parent.push('/');
                parent.push_str(dir);
                if !self.transfer.files.directories.contains(&parent) {
                    self.transfer.files.directories.push(parent.clone());
                }
            }
            let rep = match src_reader.comm.getattr(proto::files::RequestGetAttr {
                path: entry.clone(),
            }) {
                Ok(rep) => rep,
                Err(err) => {
                    error!("get attr '{}' err '{}'", entry, err);
                    self.transfer.files.errors.push(entry);
                    continue;
                }
            };
            match FileType::try_from(rep.ftype) {
                Ok(FileType::Regular) => {
                    if max_file_size.is_some_and(|size| rep.size > size) {
                        error!("{} too large ({}B)", entry, rep.size);
                        self.transfer.files.errors.push(entry.clone());
                    } else if rules.match_all(&entry) {
                        self.transfer.files.filtered.push(entry.clone())
                    } else {
                        self.transfer.files.files.push(entry.clone());
                        files_size += rep.size;
                    }
                    all_entries.insert(entry.clone());
                }
                Ok(FileType::Directory) => {
                    if !entry.is_empty() && !rules.match_all(&entry) {
                        self.transfer.files.directories.push(entry.clone());
                    } else {
                        self.transfer.files.filtered.push(entry.clone());
                    }
                    all_entries.insert(entry.clone());
                    match src_reader.comm.readdir(proto::files::RequestReadDir {
                        path: entry.clone(),
                    }) {
                        Ok(rep) => {
                            rep.filesinfo
                                .iter()
                                .for_each(|file| self.selected.push_back(file.path.clone()));
                        }
                        Err(err) => {
                            error!("get attr '{}' err '{}'", &entry, err);
                            self.transfer.files.errors.push(entry);
                            continue;
                        }
                    }
                }
                _ => self.transfer.files.errors.push(entry),
            }
        }
        self.transfer.files.files.sort();
        self.transfer.files.directories.sort();
        self.transfer.files.errors.sort();
        Ok(files_size)
    }

    fn tar_src_files(
        &mut self,
        comm: &mut impl ProtoRespUsbsas,
        children: &mut Children,
        total_size: u64,
    ) -> Result<()> {
        trace!("tar src files");
        let mut current: u64 = 0;
        for path in self
            .transfer
            .files
            .directories
            .iter()
            .chain(self.transfer.files.files.iter())
        {
            if let Err(err) = self.file_to_tar(comm, children, path, &mut current, total_size) {
                error!("Couldn't copy file '{}': {}", path, err);
                self.transfer.files.errors.push(path.clone());
            };
        }
        let report = self.transfer.to_report("success");
        self.write_report(
            &mut children.files2tar,
            &report,
            String::from("config.json"),
        )?;
        children
            .files2tar
            .comm
            .close(proto::writedst::RequestClose {})?;
        comm.status(current, total_size, true, Status::ReadSrc)?;
        Ok(())
    }

    fn file_to_tar(
        &self,
        comm: &mut impl ProtoRespUsbsas,
        children: &mut Children,
        path: &str,
        current: &mut u64,
        total_size: u64,
    ) -> Result<()> {
        let src_reader: &mut UsbsasChild<ComRqFiles> = match self.transfer.src {
            Device::Usb(_) => &mut children.scsi2files,
            _ => unimplemented!(),
        };
        let mut attrs = src_reader
            .comm
            .getattr(proto::files::RequestGetAttr { path: path.into() })?;
        // Some FS (like ext4) have a directory size != 0, set it to 0 for
        // consistency with other FS (metadata is already taken into account)
        if matches!(FileType::try_from(attrs.ftype), Ok(FileType::Directory)) {
            attrs.size = 0;
        }
        children
            .files2tar
            .comm
            .newfile(proto::writedst::RequestNewFile {
                path: path.into(),
                size: attrs.size,
                ftype: attrs.ftype,
                timestamp: attrs.timestamp,
            })?;
        let mut offset: u64 = 0;
        while attrs.size > 0 {
            let size_todo = if attrs.size < READ_FILE_MAX_SIZE {
                attrs.size
            } else {
                READ_FILE_MAX_SIZE
            };
            let rep = src_reader.comm.readfile(proto::files::RequestReadFile {
                path: path.into(),
                offset,
                size: size_todo,
            })?;
            children
                .files2tar
                .comm
                .writefile(proto::writedst::RequestWriteFile {
                    path: path.into(),
                    offset,
                    data: rep.data,
                })?;
            offset += size_todo;
            attrs.size -= size_todo;
            *current += size_todo;
            comm.status(*current, total_size, false, Status::ReadSrc)?;
        }
        children
            .files2tar
            .comm
            .endfile(proto::writedst::RequestEndFile { path: path.into() })?;
        Ok(())
    }
}

pub struct AnalyzeState {
    config: Config,
    transfer: Transfer,
}

impl RunState for AnalyzeState {
    fn run(mut self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<State> {
        trace!("analyze transfer");
        children
            .analyzer
            .comm
            .analyze(proto::analyzer::RequestAnalyze {
                id: self.transfer.userid.clone(),
            })?;

        loop {
            let status = children.analyzer.comm.recv_status()?;
            comm.status(
                status.current,
                status.total,
                status.done,
                status.status.try_into()?,
            )?;
            if status.done {
                break;
            }
        }
        comm.status(0, 0, false, Status::Analyze)?;
        let report = children
            .analyzer
            .comm
            .report(proto::analyzer::RequestReport {})?
            .report;

        if let Some(ref report) = report {
            match report.version {
                Some(2) => self.transfer.files.files.retain(|x| {
                    if let Some(file_status) = report.files.get(x.trim_start_matches('/')) {
                        match file_status.status.as_str() {
                            "CLEAN" => true,
                            "DIRTY" => {
                                self.transfer.files.dirty.push(x.to_string());
                                false
                            }
                            _ => {
                                self.transfer.files.errors.push(x.to_string());
                                false
                            }
                        }
                    } else {
                        false
                    }
                }),
                _ => panic!("unsupported"),
            }
        } else {
            comm.error("Error analyzing files")?;
            let report = self.transfer.to_report("Error analyzing files");
            return Ok(State::End(EndState {
                report: Some(report),
            }));
        };

        self.transfer.analyze_report = report;
        if self.transfer.files.files.is_empty() {
            comm.error("Nothing to copy after analyzer, aborting")?;
            let report = self
                .transfer
                .to_report("Aborted, nothing to copy after analysis");
            return Ok(State::End(EndState {
                report: Some(report),
            }));
        }
        comm.done(Status::Analyze)?;
        Ok(State::WriteDstFile(WriteDstFileState {
            config: self.config,
            transfer: self.transfer,
        }))
    }
}

pub struct WriteDstFileState {
    config: Config,
    transfer: Transfer,
}

impl RunState for WriteDstFileState {
    fn run(mut self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<State> {
        if let Device::Usb(ref usbdev) = self.transfer.dst {
            if let (Some(dev_size), Some(fstype)) = (usbdev.dev_size, self.transfer.outfstype) {
                children.files2fs.comm.init(proto::writedst::RequestInit {
                    dev_size,
                    fstype: fstype.into(),
                })?;
            } else {
                unreachable!();
            }
        }

        if children.tar2files.locked {
            children.tar2files.unlock_with(1)?;
        }

        let mut current_size = 0;
        for path in self
            .transfer
            .files
            .directories
            .iter()
            // root directory already created whith filesystem
            .filter(|dir| !(dir.is_empty() || *dir == "/"))
            .chain(self.transfer.files.files.iter())
        {
            if let Err(err) = self.file_to_dst(comm, children, path, &mut current_size) {
                error!("couldn't copy file {}: {}", &path, err);
                self.transfer.files.errors.push(path.clone());
            };
        }
        if let Some(ref confreport) = self.config.report {
            if confreport.write_dest {
                let report = self.transfer.to_report("sucess");
                let (dst_writer, report_path) = match &self.transfer.dst {
                    Device::Network(_) | Device::Command(_) => {
                        (&mut children.files2cleantar, String::from("config.json"))
                    }
                    Device::Usb(_) => (
                        &mut children.files2fs,
                        format!("/usbsas-report-{}.json", report.timestamp),
                    ),
                };
                self.write_report(dst_writer, &report, report_path)?;
            }
        }
        let status = match &self.transfer.dst {
            Device::Network(_) | Device::Command(_) => {
                children
                    .files2cleantar
                    .comm
                    .close(proto::writedst::RequestClose {})?;
                Status::MkArchive
            }
            Device::Usb(_) => {
                children
                    .files2fs
                    .comm
                    .close(proto::writedst::RequestClose {})?;
                Status::MkFs
            }
        };
        comm.done(status)?;
        Ok(State::TransferDst(TransferDstState {
            config: self.config,
            transfer: self.transfer,
        }))
    }
}

impl WriteDstFileState {
    fn file_to_dst(
        &self,
        comm: &mut impl ProtoRespUsbsas,
        children: &mut Children,
        path: &str,
        current_size: &mut u64,
    ) -> Result<()> {
        let (dst_writer, status) = match &self.transfer.dst {
            Device::Network(_) | Device::Command(_) => {
                (&mut children.files2cleantar, Status::MkArchive)
            }
            Device::Usb(_) => (&mut children.files2fs, Status::MkFs),
        };
        let mut attrs = children
            .tar2files
            .comm
            .getattr(proto::files::RequestGetAttr { path: path.into() })?;
        dst_writer.comm.newfile(proto::writedst::RequestNewFile {
            path: path.into(),
            size: attrs.size,
            ftype: attrs.ftype,
            timestamp: attrs.timestamp,
        })?;
        let mut offset: u64 = 0;
        while attrs.size > 0 {
            let size_todo = if attrs.size < READ_FILE_MAX_SIZE {
                attrs.size
            } else {
                READ_FILE_MAX_SIZE
            };
            let rep = children
                .tar2files
                .comm
                .readfile(proto::files::RequestReadFile {
                    path: path.to_string(),
                    offset,
                    size: size_todo,
                })?;
            dst_writer
                .comm
                .writefile(proto::writedst::RequestWriteFile {
                    path: path.into(),
                    offset,
                    data: rep.data,
                })?;
            offset += size_todo;
            attrs.size -= size_todo;
            *current_size += size_todo;
            comm.status(
                *current_size,
                self.transfer.selected_size.unwrap_or(*current_size),
                false,
                status,
            )?;
        }
        dst_writer.comm.endfile(proto::writedst::RequestEndFile {
            path: path.to_string(),
        })?;
        Ok(())
    }
}

pub struct TransferDstState {
    config: Config,
    transfer: Transfer,
}

impl RunState for TransferDstState {
    fn run(self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<State> {
        match self.transfer.dst {
            Device::Usb(_) => self.write_fs(comm, children)?,
            Device::Network(_) => self.upload(comm, children)?,
            Device::Command(_) => self.exec_cmd(comm, children)?,
        }

        if self.config.post_copy.is_some() {
            let ftype = if matches!(self.transfer.dst, Device::Usb(_)) {
                OutFileType::Fs
            } else {
                OutFileType::Tar
            };
            children
                .cmdexec
                .comm
                .postcopyexec(proto::cmdexec::RequestPostCopyExec {
                    outfiletype: ftype.into(),
                })?;
        }

        let report = self.transfer.to_report("success");
        comm.done(Status::AllDone)?;
        info!("transfer done, waiting end");
        Ok(State::End(EndState {
            report: Some(report),
        }))
    }
}

impl TransferDstState {
    fn write_fs(&self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<()> {
        self.forward_bitvec(children)?;
        children
            .fs2dev
            .comm
            .writefs(proto::fs2dev::RequestWriteFs {})?;
        loop {
            let status = children.fs2dev.comm.recv_status()?;
            comm.status(
                status.current,
                status.total,
                status.done,
                status.status.try_into()?,
            )?;
            if status.done {
                break;
            }
        }
        Ok(())
    }

    fn upload(&self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<()> {
        if let Device::Network(ref network) = &self.transfer.dst {
            children
                .uploader
                .comm
                .upload(proto::uploader::RequestUpload {
                    id: self.transfer.userid.clone(),
                    network: Some(network.clone()),
                })?;
            loop {
                let status = children.uploader.comm.recv_status()?;
                comm.status(
                    status.current,
                    status.total,
                    status.done,
                    status.status.try_into()?,
                )?;
                if status.done {
                    break;
                }
            }
        } else {
            bail!("Destination isn't a network")
        }
        Ok(())
    }

    fn exec_cmd(&self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<()> {
        trace!("exec cmd");
        children.cmdexec.comm.exec(proto::cmdexec::RequestExec {})?;
        comm.done(Status::ExecCmd)?;
        Ok(())
    }
}

pub struct ImgDiskState {
    device: UsbDevice,
}

impl RunState for ImgDiskState {
    fn run(mut self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<State> {
        info!("image disk {}", self.device);
        comm.imgdisk(proto::usbsas::ResponseImgDisk {})?;
        let rep = children
            .scsi2files
            .comm
            .opendevice(proto::files::RequestOpenDevice {
                busnum: self.device.busnum,
                devnum: self.device.devnum,
            })?;
        let (block_size, dev_size) = (rep.block_size, rep.dev_size);
        self.device.block_size = Some(block_size);
        self.device.dev_size = Some(dev_size);
        children
            .files2fs
            .comm
            .writeraw(proto::writedst::RequestWriteRaw {})?;

        let mut todo = dev_size;
        let mut sector_count = READ_FILE_MAX_SIZE / block_size;
        let mut offset = 0;
        loop {
            if todo < READ_FILE_MAX_SIZE {
                sector_count = todo / block_size;
            }
            let rep = children
                .scsi2files
                .comm
                .readsectors(proto::files::RequestReadSectors {
                    offset,
                    count: sector_count,
                })?;
            children
                .files2fs
                .comm
                .writedata(proto::writedst::RequestWriteData { data: rep.data })?;
            offset += sector_count;
            todo -= sector_count * block_size;
            let current = offset * block_size;
            let done = current == dev_size;
            comm.status(current, dev_size, done, Status::DiskImg)?;
            if done {
                break;
            }
        }
        let report = crate::report_diskimg(self.device);
        info!("imgdisk done");
        comm.done(Status::AllDone)?;
        Ok(State::End(EndState {
            report: Some(report),
        }))
    }
}

pub struct WipeState {
    device: UsbDevice,
    quick: bool,
    outfstype: FsType,
}

impl RunState for WipeState {
    fn run(self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<State> {
        info!(
            "wipe device (serial: {}, fmt: {}, secure: {}",
            self.device.serial,
            self.outfstype.as_str_name(),
            !self.quick
        );
        debug!("wipe device: {:#?}", self.device);

        comm.wipe(proto::usbsas::ResponseWipe {})?;

        // unlock fs2dev
        children
            .fs2dev
            .unlock_with((u64::from(self.device.devnum) << 32) | u64::from(self.device.busnum))?;
        if !self.quick {
            children.fs2dev.comm.wipe(proto::fs2dev::RequestWipe {})?;
            loop {
                let status = children.fs2dev.comm.recv_status()?;
                comm.status(
                    status.current,
                    status.total,
                    status.done,
                    status.status.try_into()?,
                )?;
                if status.done {
                    break;
                }
            }
        } else {
            comm.done(Status::MkFs)?;
        }

        let dev_size = children
            .fs2dev
            .comm
            .devsize(proto::fs2dev::RequestDevSize {})?
            .size;
        children.files2fs.comm.init(proto::writedst::RequestInit {
            dev_size,
            fstype: self.outfstype.into(),
        })?;
        children
            .files2fs
            .comm
            .close(proto::writedst::RequestClose {})?;
        self.forward_bitvec(children)?;
        children
            .fs2dev
            .comm
            .writefs(proto::fs2dev::RequestWriteFs {})?;
        loop {
            let status = children.fs2dev.comm.recv_status()?;
            comm.status(
                status.current,
                status.total,
                status.done,
                status.status.try_into()?,
            )?;
            if status.done {
                break;
            }
        }
        let report = crate::report_wipe(self.device);
        info!("wipe done");
        comm.done(Status::AllDone)?;
        Ok(State::End(EndState {
            report: Some(report),
        }))
    }
}

pub struct EndState {
    pub report: Option<TransferReport>,
}

impl RunState for EndState {
    fn run(mut self, comm: &mut impl ProtoRespUsbsas, children: &mut Children) -> Result<State> {
        let mut devices = HashMap::new();
        loop {
            match comm.recv_req()? {
                Msg::Devices(req) => self
                    .devices(comm, children, req.include_alt, &mut devices)
                    .context("listing devices")?,
                Msg::Report(_) => {
                    comm.report(proto::usbsas::ResponseReport {
                        report: self.report.clone(),
                    })?;
                }
                Msg::End(_) => {
                    children.end_wait_all(comm)?;
                    break;
                }
                unxp => {
                    comm.error(format!("Unexpected request: {:?}", unxp))?;
                    continue;
                }
            };
        }
        Ok(State::Exit)
    }
}
