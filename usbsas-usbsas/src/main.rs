//! usbsas is the parent of all processes and acts like an orchestrator,
//! spawning and managing every other processes. Only usbsas can send requests
//! to its children. It doesn't do much by itself and he as well waits for
//! requests from the final application.

use log::{debug, error, info, trace, warn};
#[cfg(feature = "log-json")]
use std::sync::{Arc, RwLock};
use std::{
    collections::{HashSet, VecDeque},
    convert::TryFrom,
    io::Write,
};
use thiserror::Error;
use usbsas_comm::{protorequest, protoresponse, Comm};
use usbsas_mass_storage::UsbDevice;
use usbsas_process::{UsbsasChild, UsbsasChildSpawner};
use usbsas_proto as proto;
use usbsas_proto::{
    common::*,
    usbsas::{request::Msg, request_copy_start::Destination, request_copy_start::Source},
};
use usbsas_utils::{self, clap::UsbsasClap, READ_FILE_MAX_SIZE, TAR_DATA_DIR};

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("analyze error: {0}")]
    Analyze(String),
    #[error("download error: {0}")]
    Download(String),
    #[error("upload error: {0}")]
    Upload(String),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("process error: {0}")]
    Process(#[from] usbsas_process::Error),
    #[error("Not enough space on destination device")]
    NotEnoughSpace,
    #[error("serde_json: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
type Result<T> = std::result::Result<T, Error>;

protoresponse!(
    CommUsbsas,
    usbsas,
    end = End[ResponseEnd],
    error = Error[ResponseError],
    id = Id[ResponseId],
    devices = Devices[ResponseDevices],
    opendevice = OpenDevice[ResponseOpenDevice],
    openpartition = OpenPartition[ResponseOpenPartition],
    partitions = Partitions[ResponsePartitions],
    getattr = GetAttr[ResponseGetAttr],
    readdir = ReadDir[ResponseReadDir],
    copystart = CopyStart[ResponseCopyStart],
    copydone = CopyDone[ResponseCopyDone],
    copystatus = CopyStatus[ResponseCopyStatus],
    copystatusdone = CopyStatusDone[ResponseCopyStatusDone],
    analyzestatus = AnalyzeStatus[ResponseAnalyzeStatus],
    analyzedone = AnalyzeDone[ResponseAnalyzeDone],
    finalcopystatus = FinalCopyStatus[ResponseFinalCopyStatus],
    finalcopystatusdone = FinalCopyStatusDone[ResponseFinalCopyStatusDone],
    notenoughspace = NotEnoughSpace[ResponseNotEnoughSpace],
    nothingtocopy = NothingToCopy[ResponseNothingToCopy],
    wipe = Wipe[ResponseWipe],
    imgdisk = ImgDisk[ResponseImgDisk],
    postcopycmd = PostCopyCmd[ResponsePostCopyCmd]
);

protorequest!(
    CommFilter,
    filter,
    filterpaths = FilterPaths[RequestFilterPaths, ResponseFilterPaths],
    end = End[RequestEnd, ResponseEnd]
);

protorequest!(
    CommIdentificator,
    identificator,
    id = Id[RequestId, ResponseId],
    end = End[RequestEnd, ResponseEnd]
);

protorequest!(
    CommFs2dev,
    fs2dev,
    size = DevSize[RequestDevSize, ResponseDevSize],
    startcopy = StartCopy[RequestStartCopy, ResponseStartCopy],
    wipe = Wipe[RequestWipe, ResponseWipe],
    loadbitvec = LoadBitVec[RequestLoadBitVec, ResponseLoadBitVec],
    end = End[RequestEnd, ResponseEnd]
);

protorequest!(
    CommUsbdev,
    usbdev,
    devices = Devices[RequestDevices, ResponseDevices],
    end = End[RequestEnd, ResponseEnd]
);

protorequest!(
    CommFiles,
    files,
    opendevice = OpenDevice[RequestOpenDevice, ResponseOpenDevice],
    partitions = Partitions[RequestPartitions, ResponsePartitions],
    openpartition = OpenPartition[RequestOpenPartition, ResponseOpenPartition],
    getattr = GetAttr[RequestGetAttr, ResponseGetAttr],
    readdir = ReadDir[RequestReadDir, ResponseReadDir],
    readfile = ReadFile[RequestReadFile, ResponseReadFile],
    readsectors = ReadSectors[RequestReadSectors, ResponseReadSectors],
    end = End[RequestEnd, ResponseEnd]
);

protorequest!(
    CommWritefs,
    writefs,
    setfsinfos = SetFsInfos[RequestSetFsInfos, ResponseSetFsInfos],
    newfile = NewFile[RequestNewFile, ResponseNewFile],
    writefile = WriteFile[RequestWriteFile, ResponseWriteFile],
    endfile = EndFile[RequestEndFile, ResponseEndFile],
    close = Close[RequestClose, ResponseClose],
    bitvec = BitVec[RequestBitVec, ResponseBitVec],
    imgdisk = ImgDisk[RequestImgDisk, ResponseImgDisk],
    writedata = WriteData[RequestWriteData, ResponseWriteData],
    end = End[RequestEnd, ResponseEnd]
);

protorequest!(
    CommWritetar,
    writetar,
    newfile = NewFile[RequestNewFile, ResponseNewFile],
    writefile = WriteFile[RequestWriteFile, ResponseWriteFile],
    endfile = EndFile[RequestEndFile, ResponseEndFile],
    close = Close[RequestClose, ResponseClose],
    end = End[RequestEnd, ResponseEnd]
);

protorequest!(
    CommUploader,
    uploader,
    upload = Upload[RequestUpload, ResponseUpload],
    end = End[RequestEnd, ResponseEnd]
);

protorequest!(
    CommDownloader,
    downloader,
    download = Download[RequestDownload, ResponseDownload],
    archiveinfos = ArchiveInfos[RequestArchiveInfos, ResponseArchiveInfos],
    end = End[RequestEnd, ResponseEnd]
);

protorequest!(
    CommCmdExec,
    cmdexec,
    exec = Exec[RequestExec, ResponseExec],
    postcopyexec = PostCopyExec[RequestPostCopyExec, ResponsePostCopyExec],
    end = End[RequestEnd, ResponseEnd]
);

protorequest!(
    CommAnalyzer,
    analyzer,
    analyze = Analyze[RequestAnalyze, ResponseAnalyze],
    end = End[RequestEnd, ResponseEnd]
);

enum State {
    Init(InitState),
    DevOpened(DevOpenedState),
    PartitionOpened(PartitionOpenedState),
    CopyFiles(CopyFilesState),
    DownloadTar(DownloadTarState),
    WriteFiles(WriteFilesState),
    UploadOrCmd(UploadOrCmdState),
    TransferDone(TransferDoneState),
    Wipe(WipeState),
    ImgDisk(ImgDiskState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut Comm<proto::usbsas::Request>, children: &mut Children) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm, children),
            State::DevOpened(s) => s.run(comm, children),
            State::PartitionOpened(s) => s.run(comm, children),
            State::CopyFiles(s) => s.run(comm, children),
            State::DownloadTar(s) => s.run(comm, children),
            State::WriteFiles(s) => s.run(comm, children),
            State::UploadOrCmd(s) => s.run(comm, children),
            State::TransferDone(s) => s.run(comm, children),
            State::Wipe(s) => s.run(comm, children),
            State::ImgDisk(s) => s.run(comm, children),
            State::WaitEnd(s) => s.run(comm, children),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {}

impl InitState {
    fn run(
        mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<State> {
        let mut id: Option<String> = None;
        loop {
            let req: proto::usbsas::Request = comm.recv()?;
            let res = match req.msg.ok_or(Error::BadRequest)? {
                Msg::Id(_) => children.id(comm, &mut id),
                Msg::Devices(_) => self.devices(comm, children),
                Msg::OpenDevice(req) => {
                    match self.open_device(comm, children, req.device.ok_or(Error::BadRequest)?) {
                        Ok(device) => return Ok(State::DevOpened(DevOpenedState { device, id })),
                        Err(err) => Err(err),
                    }
                }
                Msg::CopyStart(req) => {
                    trace!("Received CopyStart while in init state, expect export transfer");
                    if let Some(ref id_str) = id {
                        match req.source.ok_or(Error::BadRequest)? {
                            Source::SrcNet(src) => {
                                return Ok(State::DownloadTar(DownloadTarState {
                                    id: id_str.clone(),
                                    destination: req.destination.ok_or(Error::BadRequest)?,
                                    bundle_path: src.pin.to_string(),
                                }))
                            }
                            _ => {
                                log::error!("CopyStart req not export in init state");
                                Err(Error::BadRequest)
                            }
                        }
                    } else {
                        error!("empty id");
                        Err(Error::BadRequest)
                    }
                }
                Msg::Wipe(req) => {
                    return Ok(State::Wipe(WipeState {
                        busnum: req.busnum as u64,
                        devnum: req.devnum as u64,
                        quick: req.quick,
                        fstype: req.fstype,
                    }))
                }
                Msg::ImgDisk(req) => {
                    match self.open_device(comm, children, req.device.ok_or(Error::BadRequest)?) {
                        Ok(device) => return Ok(State::ImgDisk(ImgDiskState { device })),
                        Err(err) => Err(err),
                    }
                }
                Msg::End(_) => {
                    children.end_wait_all(comm)?;
                    break;
                }
                _ => Err(Error::BadRequest),
            };
            if let Err(err) = res {
                error!("{}", err);
                comm.error(proto::usbsas::ResponseError {
                    err: format!("{err}"),
                })?;
            }
        }
        Ok(State::End)
    }

    fn devices(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<()> {
        trace!("req devices");
        comm.devices(proto::usbsas::ResponseDevices {
            devices: children
                .usbdev
                .comm
                .devices(proto::usbdev::RequestDevices {})?
                .devices,
        })?;
        Ok(())
    }

    fn open_device(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
        dev_req: Device,
    ) -> Result<UsbDevice> {
        trace!("req opendevice");
        let device = children
            .scsi2files
            .comm
            .opendevice(proto::files::RequestOpenDevice {
                busnum: dev_req.busnum,
                devnum: dev_req.devnum,
            })?;
        comm.opendevice(proto::usbsas::ResponseOpenDevice {
            sector_size: device.block_size,
            dev_size: device.dev_size,
        })?;
        Ok(UsbDevice {
            busnum: dev_req.busnum,
            devnum: dev_req.devnum,
            vendorid: dev_req.vendorid,
            productid: dev_req.productid,
            manufacturer: dev_req.manufacturer,
            serial: dev_req.serial,
            description: dev_req.description,
            sector_size: u32::try_from(device.block_size)?,
            dev_size: device.dev_size,
        })
    }
}

struct DevOpenedState {
    device: UsbDevice,
    id: Option<String>,
}

impl DevOpenedState {
    fn run(
        mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<State> {
        loop {
            let req: proto::usbsas::Request = comm.recv()?;
            let res = match req.msg.ok_or(Error::BadRequest)? {
                Msg::Id(_) => children.id(comm, &mut self.id),
                Msg::Partitions(_) => self.partitions(comm, children),
                Msg::OpenPartition(req) => match self.open_partition(comm, children, req.index) {
                    Ok(_) => {
                        return Ok(State::PartitionOpened(PartitionOpenedState {
                            device: self.device,
                            id: self.id,
                        }))
                    }
                    Err(err) => {
                        error!("{}", err);
                        Err(err)
                    }
                },
                Msg::End(_) => {
                    children.end_wait_all(comm)?;
                    break;
                }
                _ => Err(Error::BadRequest),
            };
            if let Err(err) = res {
                error!("{}", err);
                comm.error(proto::usbsas::ResponseError {
                    err: format!("{err}"),
                })?;
            }
        }
        Ok(State::End)
    }

    fn partitions(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<()> {
        trace!("req partitions");
        comm.partitions(proto::usbsas::ResponsePartitions {
            partitions: children
                .scsi2files
                .comm
                .partitions(proto::files::RequestPartitions {})?
                .partitions,
        })?;
        Ok(())
    }

    fn open_partition(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
        index: u32,
    ) -> Result<()> {
        trace!("req open partition");
        children
            .scsi2files
            .comm
            .openpartition(proto::files::RequestOpenPartition { index })?;
        comm.openpartition(proto::usbsas::ResponseOpenPartition {})?;
        Ok(())
    }
}

struct PartitionOpenedState {
    device: UsbDevice,
    id: Option<String>,
}

impl PartitionOpenedState {
    fn run(
        mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<State> {
        loop {
            let req: proto::usbsas::Request = comm.recv()?;
            let res = match req.msg.ok_or(Error::BadRequest)? {
                Msg::Id(_) => children.id(comm, &mut self.id),
                Msg::GetAttr(req) => self.get_attr(comm, children, req.path),
                Msg::ReadDir(req) => self.read_dir(comm, children, req.path),
                Msg::CopyStart(req) => match req.source.ok_or(Error::BadRequest)? {
                    Source::SrcUsb(_) => {
                        if let Some(id) = self.id {
                            return Ok(State::CopyFiles(CopyFilesState {
                                device: self.device,
                                id,
                                selected: req.selected,
                                destination: req.destination.ok_or(Error::BadRequest)?,
                                write_report: req.write_report,
                            }));
                        } else {
                            error!("empty id");
                            Err(Error::BadRequest)
                        }
                    }
                    _ => Err(Error::BadRequest),
                },
                Msg::End(_) => {
                    children.end_wait_all(comm)?;
                    break;
                }
                _ => Err(Error::BadRequest),
            };
            if let Err(err) = res {
                error!("{}", err);
                comm.error(proto::usbsas::ResponseError {
                    err: format!("{err}"),
                })?;
            }
        }
        Ok(State::End)
    }

    fn get_attr(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
        path: String,
    ) -> Result<()> {
        trace!("req get attr: {}", &path);
        let attrs = children
            .scsi2files
            .comm
            .getattr(proto::files::RequestGetAttr { path })?;
        comm.getattr(proto::usbsas::ResponseGetAttr {
            ftype: attrs.ftype,
            size: attrs.size,
            timestamp: attrs.timestamp,
        })?;
        Ok(())
    }

    fn read_dir(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
        path: String,
    ) -> Result<()> {
        trace!("req read dir attrs: {}", &path);
        comm.readdir(proto::usbsas::ResponseReadDir {
            filesinfo: children
                .scsi2files
                .comm
                .readdir(proto::files::RequestReadDir { path })?
                .filesinfo,
        })?;
        Ok(())
    }
}

struct CopyFilesState {
    destination: Destination,
    device: UsbDevice,
    id: String,
    selected: Vec<String>,
    write_report: bool,
}

impl CopyFilesState {
    fn run(
        mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<State> {
        trace!("req copy");
        info!("Usbsas transfer for user: {}", self.id);

        let mut errors = vec![];
        let mut all_directories = vec![];
        let mut all_files = vec![];

        let total_files_size = self.selected_to_files_list(
            children,
            &mut errors,
            &mut all_files,
            &mut all_directories,
        )?;
        let mut filtered: Vec<String> = Vec::new();

        let all_files_filtered = self.filter_files(children, all_files, &mut filtered)?;
        let all_directories_filtered =
            self.filter_files(children, all_directories, &mut filtered)?;
        let mut all_entries_filtered = vec![];
        all_entries_filtered.append(&mut all_directories_filtered.clone());
        all_entries_filtered.append(&mut all_files_filtered.clone());

        // Abort if no files passed name filtering
        if all_entries_filtered.is_empty() {
            comm.nothingtocopy(proto::usbsas::ResponseNothingToCopy {
                rejected_filter: filtered,
                rejected_dirty: vec![],
            })?;
            warn!("Aborting copy, no files survived filter");
            return Ok(State::WaitEnd(WaitEndState {}));
        }
        let max_file_size = match children.check_dst_size(comm, &self.destination, total_files_size)
        {
            Ok(max_size) => max_size,
            Err(Error::NotEnoughSpace) => return Ok(State::WaitEnd(WaitEndState {})),
            Err(err) => return Err(err),
        };

        children.files2tar.comm.write_all(&[0_u8])?;
        children.files2tar.locked = false;

        comm.copystart(proto::usbsas::ResponseCopyStart { total_files_size })?;

        self.tar_src_files(
            comm,
            children,
            &all_entries_filtered,
            &mut errors,
            max_file_size,
        )?;

        match self.destination {
            Destination::Usb(usb) => {
                children.tar2files.comm.write_all(&[1_u8])?;
                children.tar2files.locked = false;
                Ok(State::WriteFiles(WriteFilesState {
                    directories: all_directories_filtered,
                    dirty: Vec::new(),
                    errors,
                    files: all_files_filtered,
                    filtered,
                    id: self.id,
                    usb,
                    analyze: true,
                    write_report: self.write_report,
                }))
            }
            Destination::Net(_) | Destination::Cmd(_) => {
                children.tar2files.comm.write_all(&[0_u8])?;
                children.tar2files.locked = false;
                Ok(State::UploadOrCmd(UploadOrCmdState {
                    errors,
                    filtered,
                    id: self.id,
                    destination: self.destination,
                }))
            }
        }
    }

    /// Expand tree of selected files and directories and compute total files size
    fn selected_to_files_list(
        &mut self,
        children: &mut Children,
        errors: &mut Vec<String>,
        files: &mut Vec<String>,
        directories: &mut Vec<String>,
    ) -> Result<u64> {
        let mut total_size: u64 = 0;
        let mut todo = VecDeque::from(self.selected.to_vec());
        let mut all_entries = HashSet::new();
        while let Some(entry) = todo.pop_front() {
            // First add parent(s) of file if not selected
            let mut parts = entry.trim_start_matches('/').split('/');
            // Remove last (file basename)
            let _ = parts.next_back();
            let mut parent = String::from("");
            for dir in parts {
                parent.push('/');
                parent.push_str(dir);
                if !directories.contains(&parent) {
                    directories.push(parent.clone());
                }
            }
            let rep = match children
                .scsi2files
                .comm
                .getattr(proto::files::RequestGetAttr {
                    path: entry.clone(),
                }) {
                Ok(rep) => rep,
                Err(_) => {
                    errors.push(entry);
                    continue;
                }
            };
            match FileType::from_i32(rep.ftype) {
                Some(FileType::Regular) => {
                    if all_entries.insert(entry.clone()) {
                        files.push(entry);
                        total_size += rep.size;
                    }
                }
                Some(FileType::Directory) => {
                    let mut todo_dir = VecDeque::from(vec![entry]);
                    while let Some(dir) = todo_dir.pop_front() {
                        if all_entries.insert(dir.clone()) {
                            directories.push(dir.clone());
                        }
                        let rep = children
                            .scsi2files
                            .comm
                            .readdir(proto::files::RequestReadDir { path: dir })?;
                        for file in rep.filesinfo.iter() {
                            match FileType::from_i32(file.ftype) {
                                Some(FileType::Regular) => {
                                    if all_entries.insert(file.path.clone()) {
                                        files.push(file.path.clone());
                                        total_size += file.size;
                                    }
                                }
                                Some(FileType::Directory) => {
                                    todo_dir.push_back(file.path.clone());
                                }
                                _ => errors.push(file.path.clone()),
                            }
                        }
                    }
                }
                _ => errors.push(entry),
            }
        }
        Ok(total_size)
    }

    fn filter_files(
        &mut self,
        children: &mut Children,
        files: Vec<String>,
        filtered: &mut Vec<String>,
    ) -> Result<Vec<String>> {
        trace!("filter files");
        let mut filtered_files: Vec<String> = Vec::new();
        let files_count = files.len();
        let rep = children
            .filter
            .comm
            .filterpaths(proto::filter::RequestFilterPaths {
                path: files.to_vec(),
            })?;
        if rep.results.len() != files_count {
            return Err(Error::Error("filter error".to_string()));
        }
        for (i, f) in files.iter().enumerate().take(files_count) {
            if rep.results[i] == proto::filter::FilterResult::PathOk as i32 {
                filtered_files.push(f.clone());
            } else {
                filtered.push(f.clone());
            }
        }
        Ok(filtered_files)
    }

    fn tar_src_files(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
        selected: &[String],
        errors: &mut Vec<String>,
        max_file_size: Option<u64>,
    ) -> Result<()> {
        trace!("tar src files");
        for path in selected {
            if let Err(err) = self.file_to_tar(comm, children, path, max_file_size) {
                error!("Couldn't copy file {}: {}", &path, err);
                errors.push(path.clone());
            };
        }
        children
            .files2tar
            .comm
            .close(proto::writetar::RequestClose {
                id: self.id.clone(),
                vendorid: self.device.vendorid,
                productid: self.device.productid,
                manufacturer: self.device.manufacturer.clone(),
                serial: self.device.serial.clone(),
                description: self.device.description.clone(),
            })?;
        comm.copystatusdone(proto::usbsas::ResponseCopyStatusDone {})?;
        Ok(())
    }

    fn file_to_tar(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
        path: &str,
        max_file_size: Option<u64>,
    ) -> Result<()> {
        let mut attrs = children
            .scsi2files
            .comm
            .getattr(proto::files::RequestGetAttr { path: path.into() })?;

        if let Some(max_size) = max_file_size {
            if attrs.size > max_size {
                error!(
                    "File '{}' is larger ({}B) than max size ({}B)",
                    &path, attrs.size, max_size
                );
                return Err(Error::Error("file too large".into()));
            }
        }

        // Some FS (like ext4) have a directory size != 0, fix it here for the tar archive.
        if let Some(FileType::Directory) = FileType::from_i32(attrs.ftype) {
            attrs.size = 0;
        }

        children
            .files2tar
            .comm
            .newfile(proto::writetar::RequestNewFile {
                path: path.to_string(),
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
                .scsi2files
                .comm
                .readfile(proto::files::RequestReadFile {
                    path: path.to_string(),
                    offset,
                    size: size_todo,
                })?;
            children
                .files2tar
                .comm
                .writefile(proto::writetar::RequestWriteFile {
                    path: path.to_string(),
                    offset,
                    data: rep.data,
                })?;
            offset += size_todo;
            attrs.size -= size_todo;
            comm.copystatus(proto::usbsas::ResponseCopyStatus {
                current_size: size_todo,
            })?;
        }

        children
            .files2tar
            .comm
            .endfile(proto::writetar::RequestEndFile {
                path: path.to_string(),
            })?;

        Ok(())
    }
}

struct DownloadTarState {
    destination: Destination,
    id: String,
    bundle_path: String,
}

impl DownloadTarState {
    fn run(
        mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<State> {
        trace!("req download tar");
        info!("Usbsas export for user: {}", self.id);

        let mut errors = vec![];
        let mut all_directories = vec![];
        let mut all_files = vec![];
        let remote_path = self.id.clone() + "/" + &self.bundle_path;

        let total_files_size = children
            .downloader
            .comm
            .archiveinfos(proto::downloader::RequestArchiveInfos {
                id: remote_path.clone(),
            })?
            .size;
        let max_file_size = match children.check_dst_size(comm, &self.destination, total_files_size)
        {
            Ok(max_size) => max_size,
            Err(Error::NotEnoughSpace) => return Ok(State::WaitEnd(WaitEndState {})),
            Err(err) => return Err(err),
        };

        children.files2tar.comm.write_all(&[1_u8])?;
        children.files2tar.locked = false;
        comm.copystart(proto::usbsas::ResponseCopyStart { total_files_size })?;
        self.download_tar(comm, children, &remote_path)?;
        children.tar2files.comm.write_all(&[1_u8])?;
        children.tar2files.locked = false;
        self.tar_to_files_list(
            children,
            &mut errors,
            &mut all_files,
            &mut all_directories,
            max_file_size,
        )?;

        match self.destination {
            Destination::Usb(usb) => Ok(State::WriteFiles(WriteFilesState {
                directories: all_directories,
                dirty: Vec::new(),
                errors,
                files: all_files,
                filtered: Vec::new(),
                id: self.id,
                usb,
                analyze: false,
                write_report: false,
            })),
            Destination::Net(_) | Destination::Cmd(_) => {
                children.tar2files.comm.write_all(&[0_u8])?;
                children.tar2files.locked = false;
                Ok(State::UploadOrCmd(UploadOrCmdState {
                    errors,
                    filtered: Vec::new(),
                    id: self.id,
                    destination: self.destination,
                }))
            }
        }
    }

    fn download_tar(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
        remote_path: &String,
    ) -> Result<()> {
        use proto::downloader::response::Msg;
        trace!("download tar file");
        children.downloader.comm.send(proto::downloader::Request {
            msg: Some(proto::downloader::request::Msg::Download(
                proto::downloader::RequestDownload {
                    id: remote_path.to_string(),
                },
            )),
        })?;

        loop {
            let rep: proto::downloader::Response = children.downloader.comm.recv()?;
            match rep.msg.ok_or(Error::BadRequest)? {
                Msg::DownloadStatus(status) => {
                    log::debug!("status: {}/{}", status.current_size, status.total_size);
                    continue;
                }
                Msg::Download(_) => {
                    break;
                }
                Msg::Error(err) => {
                    log::error!("Download error: {:?}", err);
                    return Err(Error::Download(err.err));
                }
                _ => {
                    log::error!("bad resp");
                    return Err(Error::BadRequest);
                }
            }
        }

        log::debug!("Bundle successfully downloaded");
        comm.copystatusdone(proto::usbsas::ResponseCopyStatusDone {})?;
        Ok(())
    }

    /// Extract all files names / paths from tar bundle
    fn tar_to_files_list(
        &mut self,
        children: &mut Children,
        errors: &mut Vec<String>,
        files: &mut Vec<String>,
        directories: &mut Vec<String>,
        max_file_size: Option<u64>,
    ) -> Result<u64> {
        let mut total_size: u64 = 0;
        let mut todo = VecDeque::from([String::from("")]);
        let mut all_entries = HashSet::new();
        while let Some(entry) = todo.pop_front() {
            if entry == "config.json" {
                continue;
            }
            let mut parts = entry.trim_start_matches('/').split('/');
            // Remove last (file basename)
            let _ = parts.next_back();
            let mut parent = String::from("");
            for dir in parts {
                parent.push('/');
                parent.push_str(dir);
                if !directories.contains(&parent) {
                    directories.push(parent.clone());
                }
            }
            let rep = match children
                .tar2files
                .comm
                .getattr(proto::files::RequestGetAttr {
                    path: entry.clone(),
                }) {
                Ok(rep) => rep,
                Err(_) => {
                    errors.push(entry);
                    continue;
                }
            };
            match FileType::from_i32(rep.ftype) {
                Some(FileType::Regular) => {
                    if !all_entries.contains(&entry) {
                        let file_too_large = match max_file_size {
                            Some(m) => rep.size > m,
                            None => false,
                        };
                        if file_too_large {
                            error!("File '{}' is too large", &entry);
                            errors.push(String::from("/") + &entry);
                        } else {
                            files.push(String::from("/") + &entry);
                            total_size += rep.size;
                        }
                        all_entries.insert(entry);
                    }
                }
                Some(FileType::Directory) => {
                    if !all_entries.contains(&entry) {
                        if !entry.is_empty() {
                            directories.push(String::from("/") + &entry);
                        }
                        all_entries.insert(entry.clone());
                    }
                    let rep = children
                        .tar2files
                        .comm
                        .readdir(proto::files::RequestReadDir { path: entry })?;
                    for file in rep.filesinfo.iter() {
                        todo.push_back(file.path.clone());
                    }
                }
                _ => errors.push(entry),
            }
        }
        files.sort();
        directories.sort();
        Ok(total_size)
    }
}

struct WriteFilesState {
    directories: Vec<String>,
    dirty: Vec<String>,
    errors: Vec<String>,
    files: Vec<String>,
    filtered: Vec<String>,
    id: String,
    usb: proto::usbsas::DestUsb,
    analyze: bool,
    write_report: bool,
}

impl WriteFilesState {
    fn run(
        mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<State> {
        let analyze_report = if self.analyze {
            self.analyze_files(comm, children)?
        } else {
            None
        };

        // Abort if no files survived antivirus
        if self.files.is_empty() {
            comm.nothingtocopy(proto::usbsas::ResponseNothingToCopy {
                rejected_filter: self.filtered,
                rejected_dirty: self.dirty,
            })?;
            warn!("Aborting copy, no files survived antivirus");
            return Ok(State::WaitEnd(WaitEndState {}));
        }

        self.init_fs(children)?;

        trace!("copy usb");

        // Create directory tree
        for dir in &self.directories {
            let timestamp = children
                .tar2files
                .comm
                .getattr(proto::files::RequestGetAttr { path: dir.clone() })?
                .timestamp;
            children
                .files2fs
                .comm
                .newfile(proto::writefs::RequestNewFile {
                    path: dir.to_string(),
                    size: 0,
                    ftype: FileType::Directory.into(),
                    timestamp,
                })?;
        }

        // Copy files
        for path in &self.files {
            let attrs = match children
                .tar2files
                .comm
                .getattr(proto::files::RequestGetAttr { path: path.clone() })
            {
                Ok(rep) => rep,
                Err(err) => {
                    error!("{}", err);
                    self.errors.push(path.clone());
                    continue;
                }
            };

            match self.write_file(
                comm,
                children,
                path,
                attrs.size,
                attrs.ftype,
                attrs.timestamp,
            ) {
                Ok(_) => (),
                Err(err) => {
                    warn!("didn't copy file {}: {}", path, err);
                    self.errors.push(path.clone());
                }
            }
        }

        if let Some(report) = analyze_report {
            if let Err(err) = self.write_report_file(children, report) {
                error!("Couldn't write report on destination fs");
                comm.error(proto::usbsas::ResponseError {
                    err: format!("err writing report on dest fs: {err}"),
                })?;
                return Ok(State::WaitEnd(WaitEndState {}));
            }
        }

        children
            .files2fs
            .comm
            .close(proto::writefs::RequestClose {})?;
        comm.copystatusdone(proto::usbsas::ResponseCopyStatusDone {})?;

        children.forward_bitvec()?;
        match self.write_fs(comm, children) {
            Ok(()) => {
                comm.copydone(proto::usbsas::ResponseCopyDone {
                    error_path: self.errors,
                    filtered_path: self.filtered,
                    dirty_path: self.dirty,
                })?;
                info!("USB TRANSFER DONE for user {}", self.id);
            }
            Err(err) => {
                comm.error(proto::usbsas::ResponseError {
                    err: format!("err writing fs: {err}"),
                })?;
                error!("USB TRANSFER FAILED for user {}", self.id);
            }
        }

        Ok(State::TransferDone(TransferDoneState {}))
    }

    fn init_fs(&mut self, children: &mut Children) -> Result<()> {
        trace!("init fs");
        let dev_size = children
            .fs2dev
            .comm
            .size(proto::fs2dev::RequestDevSize {})?
            .size;
        children
            .files2fs
            .comm
            .setfsinfos(proto::writefs::RequestSetFsInfos {
                dev_size,
                fstype: self.usb.fstype,
            })?;
        Ok(())
    }

    fn analyze_files(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<Option<serde_json::Value>> {
        trace!("analyzing files");
        use proto::analyzer::response::Msg;
        if let Some(ref mut analyzer) = children.analyzer {
            analyzer.comm.send(proto::analyzer::Request {
                msg: Some(proto::analyzer::request::Msg::Analyze(
                    proto::analyzer::RequestAnalyze {
                        id: self.id.to_string(),
                    },
                )),
            })?;

            loop {
                let rep: proto::analyzer::Response = analyzer.comm.recv()?;
                match rep.msg.ok_or(Error::BadRequest)? {
                    Msg::Analyze(res) => {
                        let report_json: serde_json::Value = serde_json::from_str(&res.report)?;
                        log::trace!("analyzer report: {:?}", report_json);
                        let files_status = report_json["files"].as_object().ok_or(Error::Error(
                            "Couldn't get files from analyzer report".into(),
                        ))?;

                        match &report_json["version"].as_u64() {
                            Some(2) => self.files.retain(|x| {
                                if let Some(status) = files_status.get(x.trim_start_matches('/')) {
                                    match status["status"].as_str() {
                                        Some("CLEAN") => true,
                                        Some("DIRTY") => {
                                            self.dirty.push(x.to_string());
                                            false
                                        }
                                        _ => {
                                            self.errors.push(x.to_string());
                                            false
                                        }
                                    }
                                } else {
                                    false
                                }
                            }),
                            _ => self.files.retain(|x| {
                                if let Some(status) = files_status
                                    .get(&format!("{TAR_DATA_DIR}/{}", x.trim_start_matches('/')))
                                {
                                    match status.as_str() {
                                        Some("CLEAN") => true,
                                        Some("DIRTY") => {
                                            self.dirty.push(format!(
                                                "/{}",
                                                x.strip_prefix(TAR_DATA_DIR)
                                                    .unwrap()
                                                    .trim_start_matches('/')
                                            ));
                                            false
                                        }
                                        _ => {
                                            self.errors.push(x.to_string());
                                            false
                                        }
                                    }
                                } else {
                                    false
                                }
                            }),
                        }

                        comm.analyzedone(proto::usbsas::ResponseAnalyzeDone {})?;
                        if self.write_report {
                            return Ok(Some(report_json));
                        } else {
                            return Ok(None);
                        }
                    }
                    Msg::UploadStatus(status) => {
                        comm.analyzestatus(proto::usbsas::ResponseAnalyzeStatus {
                            current_size: status.current_size,
                            total_size: status.total_size,
                        })?;
                        continue;
                    }
                    Msg::Error(err) => {
                        error!("{}", err.err);
                        return Err(Error::Analyze(err.err));
                    }
                    _ => return Err(Error::Analyze("Unexpected response".into())),
                }
            }
        }
        Ok(None)
    }

    fn write_file(
        &self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
        path: &str,
        size: u64,
        ftype: i32,
        timestamp: i64,
    ) -> Result<()> {
        children
            .files2fs
            .comm
            .newfile(proto::writefs::RequestNewFile {
                path: path.to_string(),
                size,
                ftype,
                timestamp,
            })?;
        let mut size = size;
        let mut offset: u64 = 0;
        while size > 0 {
            let size_todo = if size < READ_FILE_MAX_SIZE {
                size
            } else {
                READ_FILE_MAX_SIZE
            };
            let rep = match children
                .tar2files
                .comm
                .readfile(proto::files::RequestReadFile {
                    path: path.to_string(),
                    offset,
                    size: size_todo,
                }) {
                Ok(rep) => rep,
                Err(err) => {
                    return Err(Error::Error(format!("{err}")));
                }
            };
            children
                .files2fs
                .comm
                .writefile(proto::writefs::RequestWriteFile {
                    path: path.to_string(),
                    offset,
                    data: rep.data,
                })?;
            offset += size_todo;
            size -= size_todo;
            comm.copystatus(proto::usbsas::ResponseCopyStatus {
                current_size: size_todo,
            })?;
        }
        children
            .files2fs
            .comm
            .endfile(proto::writefs::RequestEndFile {
                path: path.to_string(),
            })?;
        Ok(())
    }

    fn write_report_file(&self, children: &mut Children, report: serde_json::Value) -> Result<()> {
        log::debug!("writing report");
        // Read config.json from temp archive
        let config_size = children
            .tar2files
            .comm
            .getattr(proto::files::RequestGetAttr {
                path: "config.json".into(),
            })?
            .size;
        let config_data = children
            .tar2files
            .comm
            .readfile(proto::files::RequestReadFile {
                path: "config.json".into(),
                offset: 0,
                size: config_size,
            })?
            .data;

        let config_json: serde_json::Value = serde_json::from_str(
            std::str::from_utf8(&config_data).map_err(|err| Error::Error(format!("{}", err)))?,
        )?;

        let final_report = serde_json::json!({
            "name": config_json["name"],
            "id": config_json["id"],
            "time": config_json["time"],
            "usb_src": config_json["usb_src"],
            "analyzer_report": report,
            "filtered_files": self.filtered,
            "errors": self.errors,
        });

        let report_data = serde_json::to_vec_pretty(&final_report)?;
        let report_name = format!("/usbsas-report-{}.json", config_json["time"]);

        children
            .files2fs
            .comm
            .newfile(proto::writefs::RequestNewFile {
                path: report_name.clone(),
                size: report_data.len() as u64,
                ftype: FileType::Regular.into(),
                timestamp: config_json["time"].as_f64().unwrap_or(0.0) as i64,
            })?;
        children
            .files2fs
            .comm
            .writefile(proto::writefs::RequestWriteFile {
                path: report_name.clone(),
                offset: 0,
                data: report_data,
            })?;
        children
            .files2fs
            .comm
            .endfile(proto::writefs::RequestEndFile { path: report_name })?;
        Ok(())
    }

    fn write_fs(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<()> {
        use proto::fs2dev::response::Msg;
        children
            .fs2dev
            .comm
            .startcopy(proto::fs2dev::RequestStartCopy {})?;
        loop {
            let rep: proto::fs2dev::Response = children.fs2dev.comm.recv()?;
            match rep.msg.ok_or(Error::BadRequest)? {
                Msg::CopyStatus(status) => {
                    comm.finalcopystatus(proto::usbsas::ResponseFinalCopyStatus {
                        current_size: status.current_size,
                        total_size: status.total_size,
                    })?;
                }
                Msg::CopyStatusDone(_) => {
                    comm.finalcopystatusdone(proto::usbsas::ResponseFinalCopyStatusDone {})?;
                    break;
                }
                Msg::Error(msg) => return Err(Error::Error(msg.err)),
                _ => return Err(Error::Error("error writing fs".into())),
            }
        }
        Ok(())
    }
}

struct UploadOrCmdState {
    destination: Destination,
    errors: Vec<String>,
    filtered: Vec<String>,
    id: String,
}

impl UploadOrCmdState {
    fn run(
        mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<State> {
        match &self.destination {
            Destination::Usb(_) => unreachable!("already handled"),
            Destination::Net(dest_net) => self.upload_files(comm, children, dest_net.clone())?,
            Destination::Cmd(_) => {
                trace!("exec cmd");
                children.cmdexec.comm.exec(proto::cmdexec::RequestExec {})?;
            }
        }

        // Unlock fs2dev so it can exit
        children.fs2dev.comm.write_all(&(0_u64).to_ne_bytes())?;
        children.fs2dev.locked = false;

        comm.finalcopystatusdone(proto::usbsas::ResponseFinalCopyStatusDone {})?;
        comm.copydone(proto::usbsas::ResponseCopyDone {
            error_path: self.errors,
            filtered_path: self.filtered,
            dirty_path: Vec::new(),
        })?;

        info!("NET TRANSFER DONE for user {}", self.id);
        Ok(State::TransferDone(TransferDoneState {}))
    }

    fn upload_files(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
        dstnet: proto::common::DestNet,
    ) -> Result<()> {
        use proto::uploader::response::Msg;
        trace!("upload bundle");
        children.uploader.comm.send(proto::uploader::Request {
            msg: Some(proto::uploader::request::Msg::Upload(
                proto::uploader::RequestUpload {
                    id: self.id.clone(),
                    dstnet: Some(dstnet),
                },
            )),
        })?;

        loop {
            let rep: proto::uploader::Response = children.uploader.comm.recv()?;
            match rep.msg.ok_or(Error::BadRequest)? {
                Msg::UploadStatus(status) => {
                    comm.finalcopystatus(proto::usbsas::ResponseFinalCopyStatus {
                        current_size: status.current_size,
                        total_size: status.total_size,
                    })?;
                }
                Msg::Upload(_) => {
                    debug!("files uploaded");
                    break;
                }
                Msg::Error(err) => {
                    error!("Upload error: {:?}", err);
                    return Err(Error::Upload(err.err));
                }
                _ => {
                    error!("bad resp");
                    return Err(Error::BadRequest);
                }
            }
        }

        Ok(())
    }
}

struct WipeState {
    busnum: u64,
    devnum: u64,
    quick: bool,
    fstype: i32,
}

impl WipeState {
    fn run(
        self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<State> {
        use proto::fs2dev::response::Msg;
        trace!("req wipe");

        // Unlock fs2dev
        children
            .fs2dev
            .comm
            .write_all(&((self.devnum << 32) | self.busnum).to_ne_bytes())?;
        children.fs2dev.locked = false;

        if !self.quick {
            trace!("secure wipe");
            children.fs2dev.comm.wipe(proto::fs2dev::RequestWipe {})?;
            loop {
                let rep: proto::fs2dev::Response = children.fs2dev.comm.recv()?;
                match rep.msg.ok_or(Error::BadRequest)? {
                    Msg::CopyStatus(status) => {
                        comm.finalcopystatus(proto::usbsas::ResponseFinalCopyStatus {
                            current_size: status.current_size,
                            total_size: status.total_size,
                        })?
                    }
                    Msg::CopyStatusDone(_) => break,
                    _ => {
                        return Err(Error::Error("fs2dev err while wiping".into()));
                    }
                }
            }
        }

        comm.finalcopystatusdone(proto::usbsas::ResponseFinalCopyStatusDone {})?;

        let dev_size = children
            .fs2dev
            .comm
            .size(proto::fs2dev::RequestDevSize {})?
            .size;
        children
            .files2fs
            .comm
            .setfsinfos(proto::writefs::RequestSetFsInfos {
                dev_size,
                fstype: self.fstype,
            })?;
        children
            .files2fs
            .comm
            .close(proto::writefs::RequestClose {})?;
        children.forward_bitvec()?;

        children
            .fs2dev
            .comm
            .startcopy(proto::fs2dev::RequestStartCopy {})?;
        loop {
            let rep: proto::fs2dev::Response = children.fs2dev.comm.recv()?;
            match rep.msg.ok_or(Error::BadRequest)? {
                Msg::CopyStatus(status) => {
                    comm.finalcopystatus(proto::usbsas::ResponseFinalCopyStatus {
                        current_size: status.current_size,
                        total_size: status.total_size,
                    })?;
                }
                Msg::CopyStatusDone(_) => {
                    comm.wipe(proto::usbsas::ResponseWipe {})?;
                    info!(
                        "WIPE DONE (bus/devnum: {}/{} - quick: {})",
                        self.busnum, self.devnum, self.quick
                    );
                    break;
                }
                _ => {
                    error!("bad response");
                    comm.error(proto::usbsas::ResponseError {
                        err: "bad response received from fs2dev".into(),
                    })?;
                    break;
                }
            }
        }
        Ok(State::WaitEnd(WaitEndState {}))
    }
}

struct ImgDiskState {
    device: UsbDevice,
}

impl ImgDiskState {
    fn run(
        self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<State> {
        trace!("Image disk");
        self.image_disk(comm, children)?;
        comm.imgdisk(proto::usbsas::ResponseImgDisk {})?;
        Ok(State::WaitEnd(WaitEndState {}))
    }

    fn image_disk(
        &self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<()> {
        children
            .files2fs
            .comm
            .imgdisk(proto::writefs::RequestImgDisk {})?;

        let mut todo = self.device.dev_size;
        let mut sector_count: u64 = READ_FILE_MAX_SIZE / self.device.sector_size as u64;
        let mut offset = 0;

        while todo != 0 {
            if todo < READ_FILE_MAX_SIZE {
                sector_count = todo / self.device.sector_size as u64;
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
                .writedata(proto::writefs::RequestWriteData { data: rep.data })?;
            offset += sector_count;
            todo -= sector_count * self.device.sector_size as u64;
            comm.finalcopystatus(proto::usbsas::ResponseFinalCopyStatus {
                current_size: offset * self.device.sector_size as u64,
                total_size: self.device.dev_size,
            })?;
        }
        info!("DISK IMAGE DONE");
        Ok(())
    }
}

struct TransferDoneState {}

impl TransferDoneState {
    fn run(
        self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<State> {
        let req: proto::usbsas::Request = comm.recv()?;
        match req.msg.ok_or(Error::BadRequest)? {
            Msg::End(_) => {
                children.end_wait_all(comm)?;
                return Ok(State::End);
            }
            Msg::PostCopyCmd(req) => {
                trace!("post copy cmd");
                match children
                    .cmdexec
                    .comm
                    .postcopyexec(proto::cmdexec::RequestPostCopyExec {
                        outfiletype: req.outfiletype,
                    }) {
                    Ok(_) => {
                        comm.postcopycmd(proto::usbsas::ResponsePostCopyCmd {})?;
                    }
                    Err(err) => {
                        error!("post copy cmd error: {}", err);
                        comm.error(proto::usbsas::ResponseError {
                            err: format!("{err}"),
                        })?;
                    }
                }
            }
            _ => {
                error!("bad req");
                comm.error(proto::usbsas::ResponseError {
                    err: "bad req".into(),
                })?;
            }
        }
        Ok(State::WaitEnd(WaitEndState {}))
    }
}

struct WaitEndState {}

impl WaitEndState {
    fn run(
        self,
        comm: &mut Comm<proto::usbsas::Request>,
        children: &mut Children,
    ) -> Result<State> {
        loop {
            let req: proto::usbsas::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::End(_) => {
                    children.end_wait_all(comm)?;
                    break;
                }
                _ => {
                    error!("bad req");
                    comm.error(proto::usbsas::ResponseError {
                        err: "bad req".into(),
                    })?;
                    continue;
                }
            }
        }
        Ok(State::End)
    }
}

struct Children {
    analyzer: Option<UsbsasChild<proto::analyzer::Request>>,
    identificator: UsbsasChild<proto::identificator::Request>,
    cmdexec: UsbsasChild<proto::cmdexec::Request>,
    downloader: UsbsasChild<proto::downloader::Request>,
    files2fs: UsbsasChild<proto::writefs::Request>,
    files2tar: UsbsasChild<proto::writetar::Request>,
    filter: UsbsasChild<proto::filter::Request>,
    fs2dev: UsbsasChild<proto::fs2dev::Request>,
    scsi2files: UsbsasChild<proto::files::Request>,
    tar2files: UsbsasChild<proto::files::Request>,
    uploader: UsbsasChild<proto::uploader::Request>,
    usbdev: UsbsasChild<proto::usbdev::Request>,
}

// Functions shared by multiple states are implementend on this struct.
impl Children {
    fn id(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        id: &mut Option<String>,
    ) -> Result<()> {
        trace!("req id");
        let newid = self
            .identificator
            .comm
            .id(proto::identificator::RequestId {})?
            .id;
        if !newid.is_empty() {
            *id = Some(newid);
        }
        match id {
            Some(id) => comm.id(proto::usbsas::ResponseId { id: id.clone() })?,
            None => comm.id(proto::usbsas::ResponseId { id: "".into() })?,
        }
        Ok(())
    }

    fn forward_bitvec(&mut self) -> Result<()> {
        loop {
            let rep = self
                .files2fs
                .comm
                .bitvec(proto::writefs::RequestBitVec {})?;
            self.fs2dev
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

    // If destination is USB, check that device will have enough space to stores
    // src files.
    // Returns max size of a single file (4GB if dest is FAT, None otherwise)
    fn check_dst_size(
        &mut self,
        comm: &mut Comm<proto::usbsas::Request>,
        destination: &Destination,
        total_files_size: u64,
    ) -> Result<Option<u64>> {
        // max_file_size is 4GB if we're writing a FAT fs, None otherwise
        match destination {
            Destination::Usb(ref usb) => {
                // Unlock fs2dev to get dev_size
                self.fs2dev.comm.write_all(
                    &(((u64::from(usb.devnum)) << 32) | (u64::from(usb.busnum))).to_ne_bytes(),
                )?;
                self.fs2dev.locked = false;
                let dev_size = self
                    .fs2dev
                    .comm
                    .size(proto::fs2dev::RequestDevSize {})?
                    .size;
                // Check dest dev is large enough
                // XXX try to be more precise about this
                if total_files_size > (dev_size * 98 / 100) {
                    comm.notenoughspace(proto::usbsas::ResponseNotEnoughSpace {
                        max_size: dev_size,
                    })?;
                    error!("Aborting, dest dev too small");
                    return Err(Error::NotEnoughSpace);
                }
                match OutFsType::from_i32(usb.fstype)
                    .ok_or_else(|| Error::Error("bad fstype".into()))?
                {
                    OutFsType::Fat => Ok(Some(0xFFFF_FFFF)),
                    _ => Ok(None),
                }
            }
            Destination::Net(_) | Destination::Cmd(_) => Ok(None),
        }
    }

    fn end_all(&mut self) -> Result<()> {
        trace!("req end");
        if let Some(ref mut analyzer) = self.analyzer {
            if let Err(err) = analyzer.comm.end(proto::analyzer::RequestEnd {}) {
                error!("Couldn't end analyzer: {}", err);
            };
        };
        if let Err(err) = self
            .identificator
            .comm
            .end(proto::identificator::RequestEnd {})
        {
            error!("Couldn't end identificator: {}", err);
        };
        if let Err(err) = self.cmdexec.comm.end(proto::cmdexec::RequestEnd {}) {
            error!("Couldn't end cmdexec: {}", err);
        };
        if let Err(err) = self.downloader.comm.end(proto::downloader::RequestEnd {}) {
            error!("Couldn't end downloader: {}", err);
        };
        if let Err(err) = self.files2fs.comm.end(proto::writefs::RequestEnd {}) {
            error!("Couldn't end files2fs: {}", err);
        };
        if self.files2tar.locked {
            self.files2tar.comm.write_all(&[1_u8]).ok();
        }
        if let Err(err) = self.files2tar.comm.end(proto::writetar::RequestEnd {}) {
            error!("Couldn't end files2tar: {}", err);
        };
        if let Err(err) = self.filter.comm.end(proto::filter::RequestEnd {}) {
            error!("Couldn't end filter: {}", err);
        };
        if self.fs2dev.locked {
            self.fs2dev.comm.write_all(&(0_u64).to_ne_bytes()).ok();
        }
        if let Err(err) = self.fs2dev.comm.end(proto::fs2dev::RequestEnd {}) {
            error!("Couldn't end fs2dev: {}", err);
        };
        if let Err(err) = self.scsi2files.comm.end(proto::files::RequestEnd {}) {
            error!("Couldn't end scsi2files: {}", err);
        };
        if self.tar2files.locked {
            self.tar2files.comm.write_all(&[0_u8]).ok();
        }
        if let Err(err) = self.tar2files.comm.end(proto::files::RequestEnd {}) {
            error!("Couldn't end tar2files: {}", err);
        };
        if let Err(err) = self.uploader.comm.end(proto::uploader::RequestEnd {}) {
            error!("Couldn't end uploader: {}", err);
        };
        if let Err(err) = self.usbdev.comm.end(proto::usbdev::RequestEnd {}) {
            error!("Couldn't end usbdev: {}", err);
        };
        Ok(())
    }

    fn wait_all(&mut self) -> Result<()> {
        debug!("waiting children");
        if let Some(ref mut analyzer) = self.analyzer {
            trace!("waiting analyzer");
            if let Err(err) = analyzer.wait() {
                error!("Waiting analyzer failed: {}", err);
            };
        };
        trace!("waiting identificator");
        if let Err(err) = self.identificator.wait() {
            error!("Waiting identificator failed: {}", err);
        };
        trace!("waiting cmdexec");
        if let Err(err) = self.cmdexec.wait() {
            error!("Waiting cmdexec failed: {}", err);
        };
        trace!("waiting downloader");
        if let Err(err) = self.downloader.wait() {
            error!("Waiting downloader failed: {}", err);
        };
        trace!("waiting files2fs");
        if let Err(err) = self.files2fs.wait() {
            error!("Waiting files2fs failed: {}", err);
        };
        trace!("waiting files2tar");
        if let Err(err) = self.files2tar.wait() {
            error!("Waiting files2tar failed: {}", err);
        };
        trace!("waiting filter");
        if let Err(err) = self.filter.wait() {
            error!("Waiting filter failed: {}", err);
        };
        trace!("waiting fs2dev");
        if let Err(err) = self.fs2dev.wait() {
            error!("Waiting fs2dev failed: {}", err);
        };
        trace!("waiting scsi2files");
        if let Err(err) = self.scsi2files.wait() {
            error!("Waiting scsi2files failed: {}", err);
        };
        trace!("waiting tar2files");
        if let Err(err) = self.tar2files.wait() {
            error!("Waiting tar2files failed: {}", err);
        };
        trace!("waiting uploader");
        if let Err(err) = self.uploader.wait() {
            error!("Waiting uploader failed: {}", err);
        };
        trace!("waiting usbdev");
        if let Err(err) = self.usbdev.wait() {
            error!("Waiting usbdev failed: {}", err);
        };
        Ok(())
    }

    fn end_wait_all(&mut self, comm: &mut Comm<proto::usbsas::Request>) -> Result<()> {
        trace!("req end");
        self.end_all()?;
        self.wait_all()?;
        comm.end(proto::usbsas::ResponseEnd {})?;
        Ok(())
    }
}

pub struct Usbsas {
    comm: Comm<proto::usbsas::Request>,
    children: Children,
    state: State,
}

impl Usbsas {
    fn new(
        comm: Comm<proto::usbsas::Request>,
        config_path: &str,
        out_tar: &str,
        out_fs: &str,
        analyze: bool,
    ) -> Result<Self> {
        trace!("init");
        let mut pipes_read = vec![];
        let mut pipes_write = vec![];

        pipes_read.push(comm.input_fd());
        pipes_write.push(comm.output_fd());

        let identificator = UsbsasChildSpawner::new("usbsas-identificator")
            .spawn::<proto::identificator::Request>()?;
        pipes_read.push(identificator.comm.input_fd());
        pipes_write.push(identificator.comm.output_fd());

        let cmdexec = UsbsasChildSpawner::new("usbsas-cmdexec")
            .arg(out_tar)
            .arg(out_fs)
            .args(&["-c", config_path])
            .spawn::<proto::cmdexec::Request>()?;
        pipes_read.push(cmdexec.comm.input_fd());
        pipes_write.push(cmdexec.comm.output_fd());

        let downloader = UsbsasChildSpawner::new("usbsas-downloader")
            .arg(out_tar)
            .args(&["-c", config_path])
            .spawn::<proto::downloader::Request>()?;
        pipes_read.push(downloader.comm.input_fd());
        pipes_write.push(downloader.comm.output_fd());

        let usbdev = UsbsasChildSpawner::new("usbsas-usbdev")
            .args(&["-c", config_path])
            .spawn::<proto::usbdev::Request>()?;
        pipes_read.push(usbdev.comm.input_fd());
        pipes_write.push(usbdev.comm.output_fd());

        let scsi2files =
            UsbsasChildSpawner::new("usbsas-scsi2files").spawn::<proto::files::Request>()?;
        pipes_read.push(scsi2files.comm.input_fd());
        pipes_write.push(scsi2files.comm.output_fd());

        let files2tar = UsbsasChildSpawner::new("usbsas-files2tar")
            .arg(out_tar)
            .wait_on_startup()
            .spawn::<proto::writetar::Request>()?;
        pipes_read.push(files2tar.comm.input_fd());
        pipes_write.push(files2tar.comm.output_fd());

        let files2fs = UsbsasChildSpawner::new("usbsas-files2fs")
            .arg(out_fs)
            .spawn::<proto::writefs::Request>()?;
        pipes_read.push(files2fs.comm.input_fd());
        pipes_write.push(files2fs.comm.output_fd());

        let filter = UsbsasChildSpawner::new("usbsas-filter")
            .args(&["-c", config_path])
            .spawn::<proto::filter::Request>()?;
        pipes_read.push(filter.comm.input_fd());
        pipes_write.push(filter.comm.output_fd());

        let fs2dev = UsbsasChildSpawner::new("usbsas-fs2dev")
            .arg(out_fs)
            .wait_on_startup()
            .spawn::<proto::fs2dev::Request>()?;
        pipes_read.push(fs2dev.comm.input_fd());
        pipes_write.push(fs2dev.comm.output_fd());

        let tar2files = UsbsasChildSpawner::new("usbsas-tar2files")
            .arg(out_tar)
            .wait_on_startup()
            .spawn::<proto::files::Request>()?;
        pipes_read.push(tar2files.comm.input_fd());
        pipes_write.push(tar2files.comm.output_fd());

        let uploader = UsbsasChildSpawner::new("usbsas-uploader")
            .arg(out_tar)
            .spawn::<proto::uploader::Request>()?;
        pipes_read.push(uploader.comm.input_fd());
        pipes_write.push(uploader.comm.output_fd());

        let analyzer = if analyze {
            let analyzer = UsbsasChildSpawner::new("usbsas-analyzer")
                .arg(out_tar)
                .args(&["-c", config_path])
                .spawn::<proto::analyzer::Request>()?;
            pipes_read.push(analyzer.comm.input_fd());
            pipes_write.push(analyzer.comm.output_fd());

            Some(analyzer)
        } else {
            None
        };

        trace!("enter seccomp");
        usbsas_sandbox::usbsas::seccomp(pipes_read, pipes_write)?;

        let children = Children {
            analyzer,
            identificator,
            cmdexec,
            downloader,
            files2fs,
            files2tar,
            filter,
            fs2dev,
            scsi2files,
            tar2files,
            uploader,
            usbdev,
        };

        Ok(Usbsas {
            comm,
            children,
            state: State::Init(InitState {}),
        })
    }

    fn main_loop(self) -> Result<()> {
        let (mut comm, mut children, mut state) = (self.comm, self.children, self.state);
        loop {
            state = match state.run(&mut comm, &mut children) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}, waiting end", err);
                    comm.error(proto::usbsas::ResponseError {
                        err: format!("run error: {err}"),
                    })?;
                    State::WaitEnd(WaitEndState {})
                }
            }
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    usbsas_utils::log::init_logger();
    let matches = usbsas_utils::clap::new_usbsas_cmd("usbsas-usbsas")
        .add_config_arg()
        .add_tar_path_arg()
        .add_fs_path_arg()
        .arg(
            clap::Arg::new("analyze")
                .short('a')
                .long("analyze")
                .help("Analyze files with antivirus server")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();
    let config = matches.get_one::<String>("config").unwrap();
    let tar_path = matches.get_one::<String>("tar_path").unwrap();
    let fs_path = matches.get_one::<String>("fs_path").unwrap();

    info!("start ({}): {} {}", std::process::id(), tar_path, fs_path);
    Usbsas::new(
        Comm::from_env()?,
        config,
        tar_path,
        fs_path,
        matches.get_flag("analyze"),
    )?
    .main_loop()
    .map(|_| log::debug!("exit"))
}
