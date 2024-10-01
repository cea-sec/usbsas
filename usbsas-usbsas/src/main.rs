//! usbsas is the parent of all processes and acts like an orchestrator,
//! spawning and managing every other processes. Only usbsas can send requests
//! to its children. It doesn't do much by itself and he as well waits for
//! requests from the final application.

use log::{debug, error, info, trace, warn};
use serde_json::json;
use std::{
    collections::{HashSet, VecDeque},
    convert::TryFrom,
    env,
    fs::File,
};
use thiserror::Error;
use usbsas_comm::{
    ComRpUsbsas, ComRqAnalyzer, ComRqCmdExec, ComRqDownloader, ComRqFiles, ComRqFilter,
    ComRqFs2Dev, ComRqIdentificator, ComRqUploader, ComRqUsbDev, ComRqWriteFs, ComRqWriteTar, Comm,
    ProtoReqAnalyzer, ProtoReqCmdExec, ProtoReqCommon, ProtoReqDownloader, ProtoReqFiles,
    ProtoReqFilter, ProtoReqFs2Dev, ProtoReqIdentificator, ProtoReqUploader, ProtoReqUsbDev,
    ProtoReqWriteFs, ProtoReqWriteTar, ProtoRespCommon, ProtoRespUsbsas, SendRecv, ToFromFd,
};
use usbsas_config::{conf_parse, conf_read};
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
    #[error("Error filtering files (bad count)")]
    Filter,
    #[error("File too large")]
    FileTooLarge,
    #[error("{0}")]
    Wipe(String),
    #[error("{0}")]
    WriteFs(String),
    #[error("serde_json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
pub struct UsbMS {
    pub dev: UsbDevice,
    pub sector_size: u32,
    pub dev_size: u64,
}

impl std::fmt::Display for UsbMS {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} ({} - {})", self.dev, self.sector_size, self.dev_size)
    }
}

enum State {
    Init(InitState),
    DevOpened(DevOpenedState),
    PartitionOpened(PartitionOpenedState),
    CopyFiles(CopyFilesState),
    Analyze(AnalyzeState),
    DownloadTar(DownloadTarState),
    WriteCleanTar(WriteCleanTarState),
    WriteFs(WriteFsState),
    UploadOrCmd(UploadOrCmdState),
    TransferDone(TransferDoneState),
    Wipe(WipeState),
    ImgDisk(ImgDiskState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm, children),
            State::DevOpened(s) => s.run(comm, children),
            State::PartitionOpened(s) => s.run(comm, children),
            State::CopyFiles(s) => s.run(comm, children),
            State::Analyze(s) => s.run(comm, children),
            State::DownloadTar(s) => s.run(comm, children),
            State::WriteCleanTar(s) => s.run(comm, children),
            State::WriteFs(s) => s.run(comm, children),
            State::UploadOrCmd(s) => s.run(comm, children),
            State::TransferDone(s) => s.run(comm, children),
            State::Wipe(s) => s.run(comm, children),
            State::ImgDisk(s) => s.run(comm, children),
            State::WaitEnd(s) => s.run(comm, children),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {
    config: Config,
}

impl InitState {
    fn run(mut self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
        let mut id: Option<String> = None;
        loop {
            let req: proto::usbsas::Request = comm.recv()?;
            let res = match req.msg.ok_or(Error::BadRequest)? {
                Msg::Id(_) => children.id(comm, &mut id),
                Msg::UsbDevices(_) => self.usb_devices(comm, children),
                Msg::AltTargets(_) => self.alt_targets(comm),
                Msg::OpenDevice(req) => {
                    match self.open_device(comm, children, req.device.ok_or(Error::BadRequest)?) {
                        Ok(device) => {
                            return Ok(State::DevOpened(DevOpenedState {
                                device,
                                id,
                                config: self.config,
                            }))
                        }
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
                                    config: self.config,
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
                comm.error(err)?;
            }
        }
        Ok(State::End)
    }

    fn usb_devices(&mut self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<()> {
        trace!("req devices");
        comm.usbdevices(proto::usbsas::ResponseUsbDevices {
            devices: children
                .usbdev
                .comm
                .devices(proto::usbdev::RequestDevices {})?
                .devices,
        })?;
        Ok(())
    }

    fn alt_targets(&mut self, comm: &mut ComRpUsbsas) -> Result<()> {
        let mut alt_targets: Vec<usbsas_proto::common::AltTarget> = Vec::new();
        if let Some(dst_networks) = &self.config.dst_networks {
            for network in dst_networks {
                alt_targets.push(AltTarget {
                    target: Some(usbsas_proto::common::alt_target::Target::Network(
                        usbsas_proto::common::Network {
                            url: network.url.clone(),
                            krb_service_name: network
                                .krb_service_name
                                .clone()
                                .unwrap_or(String::from("")),
                        },
                    )),
                    descr: network.description.clone(),
                    long_descr: network.longdescr.clone(),
                    is_src: false,
                    is_dst: true,
                });
            }
        };
        if let Some(network) = &self.config.src_network {
            alt_targets.push(AltTarget {
                target: Some(usbsas_proto::common::alt_target::Target::Network(
                    usbsas_proto::common::Network {
                        url: network.url.clone(),
                        krb_service_name: network
                            .krb_service_name
                            .clone()
                            .unwrap_or(String::from("")),
                    },
                )),
                descr: network.description.clone(),
                long_descr: network.longdescr.clone(),
                is_src: true,
                is_dst: false,
            });
        };
        if let Some(cmd) = &self.config.command {
            alt_targets.push(AltTarget {
                target: Some(usbsas_proto::common::alt_target::Target::Command(
                    usbsas_proto::common::Command {
                        bin: cmd.command_bin.clone(),
                        args: cmd.command_args.clone(),
                    },
                )),
                descr: cmd.description.clone(),
                long_descr: cmd.longdescr.clone(),
                is_src: false,
                is_dst: true,
            });
        };
        comm.alttargets(proto::usbsas::ResponseAltTargets { alt_targets })?;
        Ok(())
    }

    fn open_device(
        &mut self,
        comm: &mut ComRpUsbsas,
        children: &mut Children,
        dev_req: proto::common::UsbDevice,
    ) -> Result<UsbMS> {
        info!("Opening device {}", dev_req);
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
        Ok(UsbMS {
            dev: UsbDevice {
                busnum: dev_req.busnum,
                devnum: dev_req.devnum,
                vendorid: dev_req.vendorid,
                productid: dev_req.productid,
                manufacturer: dev_req.manufacturer,
                serial: dev_req.serial,
                description: dev_req.description,
                is_src: dev_req.is_src,
                is_dst: dev_req.is_dst,
            },
            sector_size: u32::try_from(device.block_size)?,
            dev_size: device.dev_size,
        })
    }
}

struct DevOpenedState {
    device: UsbMS,
    id: Option<String>,
    config: Config,
}

impl DevOpenedState {
    fn run(mut self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
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
                            config: self.config,
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
                comm.error(err)?;
            }
        }
        Ok(State::End)
    }

    fn partitions(&mut self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<()> {
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
        comm: &mut ComRpUsbsas,
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
    device: UsbMS,
    id: Option<String>,
    config: Config,
}

impl PartitionOpenedState {
    fn run(mut self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
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
                                config: self.config,
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
                comm.error(err)?;
            }
        }
        Ok(State::End)
    }

    fn get_attr(
        &mut self,
        comm: &mut ComRpUsbsas,
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
        comm: &mut ComRpUsbsas,
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
    device: UsbMS,
    id: String,
    selected: Vec<String>,
    config: Config,
}

impl CopyFilesState {
    fn run(mut self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
        trace!("req copy");

        info!(
            "Starting transfer from {} to {:?} for user: {}",
            self.device, self.destination, self.id
        );

        let mut report = init_report()?;
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

        report["file_names"] = all_files_filtered.clone().into();
        report["filtered_files"] = filtered.clone().into();
        report["user"] = serde_json::Value::String(self.id.clone());
        report["source"] = json!({
            "vendorid": self.device.dev.vendorid,
            "productid": self.device.dev.productid,
            "manufacturer": self.device.dev.manufacturer,
            "serial": self.device.dev.serial,
            "description": self.device.dev.description
        });

        if let Destination::Usb(dest) = &self.destination {
            if let Some(out_dev) = children
                .usbdev
                .comm
                .devices(proto::usbdev::RequestDevices {})?
                .devices
                .iter()
                .find(|&dev| dev.busnum == dest.busnum && dev.devnum == dest.devnum)
            {
                report["destination"] = json!({
                    "vendorid": out_dev.vendorid,
                    "productid": out_dev.productid,
                    "manufacturer": out_dev.manufacturer,
                    "serial": out_dev.serial,
                    "description": out_dev.description
                });
            };
        };

        // Abort if no files passed name filtering and no report requested
        if all_entries_filtered.is_empty() && !self.config.write_report_dest {
            comm.nothingtocopy(proto::usbsas::ResponseNothingToCopy {
                report: serde_json::to_vec(&report)?,
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

        comm.copystart(proto::usbsas::ResponseCopyStart { total_files_size })?;

        self.tar_src_files(
            comm,
            children,
            &all_entries_filtered,
            &mut errors,
            max_file_size,
            total_files_size,
            &report,
        )?;

        let analyze = match self.destination {
            Destination::Usb(_) => self.config.analyze_usb,
            Destination::Net(_) => self.config.analyze_net,
            Destination::Cmd(_) => self.config.analyze_cmd,
        };

        if analyze {
            Ok(State::Analyze(AnalyzeState {
                directories: all_directories_filtered,
                files: all_files_filtered,
                errors,
                id: self.id,
                destination: self.destination,
                report,
                config: self.config,
            }))
        } else {
            match self.destination {
                Destination::Usb(usb) => {
                    children.uploader.unlock_with(&[0_u8])?;
                    children.cmdexec.unlock_with(&[0_u8])?;
                    children.tar2files.unlock_with(&[1_u8])?;
                    Ok(State::WriteFs(WriteFsState {
                        directories: all_directories_filtered,
                        errors,
                        files: all_files_filtered,
                        usb,
                        report,
                        config: self.config,
                    }))
                }
                Destination::Net(_) | Destination::Cmd(_) => {
                    report["error_files"] = errors.into();
                    children.uploader.unlock_with(&[1_u8])?;
                    children.cmdexec.unlock_with(&[1_u8])?;
                    children.tar2files.unlock_with(&[0_u8])?;
                    Ok(State::UploadOrCmd(UploadOrCmdState {
                        id: self.id,
                        destination: self.destination,
                        report,
                    }))
                }
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
            match FileType::try_from(rep.ftype) {
                Ok(FileType::Regular) => {
                    if all_entries.insert(entry.clone()) {
                        files.push(entry);
                        total_size += rep.size;
                    }
                }
                Ok(FileType::Directory) => {
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
                            match FileType::try_from(file.ftype) {
                                Ok(FileType::Regular) => {
                                    if all_entries.insert(file.path.clone()) {
                                        files.push(file.path.clone());
                                        total_size += file.size;
                                    }
                                }
                                Ok(FileType::Directory) => {
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
            return Err(Error::Filter);
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
        &self,
        comm: &mut ComRpUsbsas,
        children: &mut Children,
        entries_filtered: &[String],
        errors: &mut Vec<String>,
        max_file_size: Option<u64>,
        total_size: u64,
        report: &serde_json::Value,
    ) -> Result<()> {
        trace!("tar src files");
        let mut current_size: u64 = 0;
        for path in entries_filtered {
            if let Err(err) = self.file_to_tar(
                comm,
                children,
                path,
                max_file_size,
                &mut current_size,
                total_size,
            ) {
                error!("Couldn't copy file {}: {}", &path, err);
                errors.push(path.clone());
            };
        }
        children
            .files2tar
            .comm
            .close(proto::writetar::RequestClose {
                infos: serde_json::to_vec(&report)?,
            })?;
        comm.copystatusdone(proto::usbsas::ResponseCopyStatusDone {})?;
        Ok(())
    }

    fn file_to_tar(
        &self,
        comm: &mut ComRpUsbsas,
        children: &mut Children,
        path: &str,
        max_file_size: Option<u64>,
        current_size: &mut u64,
        total_size: u64,
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
                return Err(Error::FileTooLarge);
            }
        }

        // Some FS (like ext4) have a directory size != 0, fix it here for the tar archive.
        if let Ok(FileType::Directory) = FileType::try_from(attrs.ftype) {
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
            *current_size += size_todo;
            comm.status(*current_size, total_size, false)?;
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
    config: Config,
}

impl DownloadTarState {
    fn run(mut self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
        trace!("req download tar");
        info!("starting export for user: {}", self.id);

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

        comm.copystart(proto::usbsas::ResponseCopyStart { total_files_size })?;
        self.download_tar(comm, children, &remote_path)?;
        children.tar2files.unlock_with(&[1_u8])?;
        self.tar_to_files_list(
            children,
            &mut errors,
            &mut all_files,
            &mut all_directories,
            max_file_size,
        )?;

        let mut report = init_report()?;
        report["source"] = "network".into();
        report["file_names"] = all_files.clone().into();

        self.config.analyze_usb = false;
        self.config.analyze_net = false;
        self.config.analyze_cmd = false;
        self.config.write_report_dest = false;
        match self.destination {
            Destination::Usb(usb) => Ok(State::WriteFs(WriteFsState {
                directories: all_directories,
                errors,
                files: all_files,
                usb,
                report,
                config: self.config,
            })),
            Destination::Net(_) | Destination::Cmd(_) => {
                report["error_files"] = errors.into();
                children.tar2files.unlock_with(&[0_u8])?;
                Ok(State::UploadOrCmd(UploadOrCmdState {
                    id: self.id,
                    destination: self.destination,
                    report,
                }))
            }
        }
    }

    fn download_tar(
        &mut self,
        comm: &mut ComRpUsbsas,
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
                Msg::Status(status) => {
                    log::debug!("status: {}/{}", status.current, status.total);
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
            match FileType::try_from(rep.ftype) {
                Ok(FileType::Regular) => {
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
                Ok(FileType::Directory) => {
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

struct AnalyzeState {
    directories: Vec<String>,
    errors: Vec<String>,
    files: Vec<String>,
    id: String,
    destination: Destination,
    report: serde_json::Value,
    config: Config,
}

impl AnalyzeState {
    fn run(mut self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
        let mut dirty: Vec<String> = Vec::new();
        let analyze_report = self.analyze_files(comm, children, &mut dirty)?;
        self.report["analyzer_report"] = analyze_report;

        children.tar2files.unlock_with(&[1])?;

        // Abort if no files survived antivirus and no report requested
        if self.files.is_empty() && !self.config.write_report_dest {
            comm.nothingtocopy(proto::usbsas::ResponseNothingToCopy {
                report: serde_json::to_vec(&self.report)?,
            })?;
            warn!("Aborting copy, no files survived filter and antivirus");
            return Ok(State::WaitEnd(WaitEndState {}));
        }

        children.cmdexec.unlock_with(&[2])?;
        children.uploader.unlock_with(&[2])?;

        match self.destination {
            Destination::Usb(usb) => Ok(State::WriteFs(WriteFsState {
                directories: self.directories,
                errors: self.errors,
                files: self.files,
                usb,
                report: self.report,
                config: self.config,
            })),
            Destination::Net(_) | Destination::Cmd(_) => {
                Ok(State::WriteCleanTar(WriteCleanTarState {
                    directories: self.directories,
                    errors: self.errors,
                    files: self.files,
                    id: self.id,
                    destination: self.destination,
                    report: self.report,
                }))
            }
        }
    }

    fn analyze_files(
        &mut self,
        comm: &mut ComRpUsbsas,
        children: &mut Children,
        dirty: &mut Vec<String>,
    ) -> Result<serde_json::Value> {
        trace!("analyzing files");
        use proto::analyzer::response::Msg;
        children.analyzer.comm.send(proto::analyzer::Request {
            msg: Some(proto::analyzer::request::Msg::Analyze(
                proto::analyzer::RequestAnalyze {
                    id: self.id.to_string(),
                },
            )),
        })?;

        loop {
            let rep: proto::analyzer::Response = children.analyzer.comm.recv()?;
            match rep.msg.ok_or(Error::BadRequest)? {
                Msg::Analyze(res) => {
                    let report_json: serde_json::Value = serde_json::from_str(&res.report)?;
                    log::trace!("analyzer report: {:?}", report_json);
                    let files_status = report_json["files"].as_object().ok_or(Error::Analyze(
                        "Couldn't get files from analyzer report".into(),
                    ))?;

                    match &report_json["version"].as_u64() {
                        Some(2) => self.files.retain(|x| {
                            if let Some(status) = files_status.get(x.trim_start_matches('/')) {
                                match status["status"].as_str() {
                                    Some("CLEAN") => true,
                                    Some("DIRTY") => {
                                        dirty.push(x.to_string());
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
                                        dirty.push(x.to_string());
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
                    return Ok(report_json);
                }
                Msg::Status(status) => {
                    comm.status(status.current, status.total, false)?;
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
}

struct WriteCleanTarState {
    directories: Vec<String>,
    errors: Vec<String>,
    files: Vec<String>,
    id: String,
    destination: Destination,
    report: serde_json::Value,
}

impl WriteCleanTarState {
    fn run(mut self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
        trace!("write clean tar");

        let mut current_size: u64 = 0;
        for path in self.directories.iter().chain(self.files.iter()) {
            if let Err(err) = self.file_to_clean_tar(comm, children, path, &mut current_size) {
                error!("Couldn't copy file {}: {}", &path, err);
                self.errors.push(path.clone());
            };
        }

        self.report["error_files"] = self.errors.clone().into();

        children
            .files2cleantar
            .comm
            .close(proto::writetar::RequestClose {
                infos: serde_json::to_vec(&self.report)?,
            })?;

        comm.copystatusdone(proto::usbsas::ResponseCopyStatusDone {})?;

        Ok(State::UploadOrCmd(UploadOrCmdState {
            id: self.id,
            destination: self.destination,
            report: self.report,
        }))
    }

    fn file_to_clean_tar(
        &self,
        comm: &mut ComRpUsbsas,
        children: &mut Children,
        path: &str,
        current_size: &mut u64,
    ) -> Result<()> {
        let mut attrs = children
            .tar2files
            .comm
            .getattr(proto::files::RequestGetAttr { path: path.into() })?;

        children
            .files2cleantar
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
                .tar2files
                .comm
                .readfile(proto::files::RequestReadFile {
                    path: path.to_string(),
                    offset,
                    size: size_todo,
                })?;
            children
                .files2cleantar
                .comm
                .writefile(proto::writetar::RequestWriteFile {
                    path: path.to_string(),
                    offset,
                    data: rep.data,
                })?;
            offset += size_todo;
            attrs.size -= size_todo;
            *current_size += size_todo;
            comm.status(*current_size, 0, false)?;
        }

        children
            .files2cleantar
            .comm
            .endfile(proto::writetar::RequestEndFile {
                path: path.to_string(),
            })?;

        Ok(())
    }
}

struct WriteFsState {
    directories: Vec<String>,
    errors: Vec<String>,
    files: Vec<String>,
    usb: proto::usbsas::DestUsb,
    report: serde_json::Value,
    config: Config,
}

impl WriteFsState {
    fn run(mut self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
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

        let mut current_size: u64 = 0;
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
                &mut current_size,
            ) {
                Ok(_) => (),
                Err(err) => {
                    warn!("didn't copy file {}: {}", path, err);
                    self.errors.push(path.clone());
                }
            }
        }

        self.report["error_files"] = self.errors.clone().into();

        if self.config.write_report_dest {
            if let Err(err) = self.write_report_file(children) {
                error!("Couldn't write report on destination fs");
                comm.error(format!("err writing report on dest fs: {err}"))?;
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
                    report: serde_json::to_vec(&self.report)?,
                })?;
                info!("transfer done");
            }
            Err(err) => {
                comm.error(format!("err writing fs: {err}"))?;
                error!("transfer failed: {}", err);
            }
        }

        Ok(State::TransferDone(TransferDoneState {}))
    }

    fn init_fs(&mut self, children: &mut Children) -> Result<()> {
        trace!("init fs");
        let dev_size = children
            .fs2dev
            .comm
            .devsize(proto::fs2dev::RequestDevSize {})?
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

    fn write_file(
        &self,
        comm: &mut ComRpUsbsas,
        children: &mut Children,
        path: &str,
        size: u64,
        ftype: i32,
        timestamp: i64,
        current_size: &mut u64,
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
            let rep = children
                .tar2files
                .comm
                .readfile(proto::files::RequestReadFile {
                    path: path.to_string(),
                    offset,
                    size: size_todo,
                })?;
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
            *current_size += size_todo;
            comm.status(*current_size, 0, false)?;
        }
        children
            .files2fs
            .comm
            .endfile(proto::writefs::RequestEndFile {
                path: path.to_string(),
            })?;
        Ok(())
    }

    fn write_report_file(&mut self, children: &mut Children) -> Result<()> {
        log::debug!("writing report");

        let report_data = serde_json::to_vec_pretty(&self.report)?;
        let report_name = format!("/usbsas-report-{}.json", self.report["timestamp"]);

        children
            .files2fs
            .comm
            .newfile(proto::writefs::RequestNewFile {
                path: report_name.clone(),
                size: report_data.len() as u64,
                ftype: FileType::Regular.into(),
                timestamp: self.report["timestamp"].as_f64().unwrap_or(0.0) as i64,
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

    fn write_fs(&mut self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<()> {
        use proto::fs2dev::response::Msg;
        children
            .fs2dev
            .comm
            .startcopy(proto::fs2dev::RequestStartCopy {})?;
        loop {
            let rep: proto::fs2dev::Response = children.fs2dev.comm.recv()?;
            match rep.msg.ok_or(Error::BadRequest)? {
                Msg::Status(status) => {
                    comm.status(status.current, status.total, status.done)?;
                    if status.done {
                        comm.finalcopystatusdone(proto::usbsas::ResponseFinalCopyStatusDone {})?;
                        break;
                    }
                }
                Msg::Error(msg) => return Err(Error::WriteFs(msg.err)),
                _ => return Err(Error::WriteFs("error writing fs".into())),
            }
        }
        Ok(())
    }
}

struct UploadOrCmdState {
    destination: Destination,
    id: String,
    report: serde_json::Value,
}

impl UploadOrCmdState {
    fn run(mut self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
        match &self.destination {
            Destination::Usb(_) => unreachable!("already handled"),
            Destination::Net(dest_net) => self.upload_files(comm, children, dest_net.clone())?,
            Destination::Cmd(_) => {
                debug!("exec cmd");
                self.report["destination"] = "cmd".into();
                children.cmdexec.comm.exec(proto::cmdexec::RequestExec {})?;
            }
        }

        // Unlock fs2dev so it can exit
        children.fs2dev.unlock_with(&(0_u64).to_ne_bytes())?;

        comm.finalcopystatusdone(proto::usbsas::ResponseFinalCopyStatusDone {})?;
        comm.copydone(proto::usbsas::ResponseCopyDone {
            report: serde_json::to_vec(&self.report)?,
        })?;

        info!("net transfer done");
        Ok(State::TransferDone(TransferDoneState {}))
    }

    fn upload_files(
        &mut self,
        comm: &mut ComRpUsbsas,
        children: &mut Children,
        network: proto::common::Network,
    ) -> Result<()> {
        use proto::uploader::response::Msg;
        trace!("upload bundle");
        children.uploader.comm.send(proto::uploader::Request {
            msg: Some(proto::uploader::request::Msg::Upload(
                proto::uploader::RequestUpload {
                    id: self.id.clone(),
                    network: Some(network),
                },
            )),
        })?;

        loop {
            let rep: proto::uploader::Response = children.uploader.comm.recv()?;
            match rep.msg.ok_or(Error::BadRequest)? {
                Msg::Status(status) => {
                    comm.status(status.current, status.total, status.done)?;
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
        self.report["destination"] = "network".into();
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
    fn run(self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
        use proto::fs2dev::response::Msg;
        info!(
            "starting wipe {}-{} quick: {} ",
            self.busnum, self.devnum, self.quick
        );

        // Unlock fs2dev
        children
            .fs2dev
            .unlock_with(&((self.devnum << 32) | self.busnum).to_ne_bytes())?;

        if !self.quick {
            trace!("secure wipe");
            children.fs2dev.comm.wipe(proto::fs2dev::RequestWipe {})?;
            loop {
                let rep: proto::fs2dev::Response = children.fs2dev.comm.recv()?;
                match rep.msg.ok_or(Error::BadRequest)? {
                    Msg::Status(status) => {
                        comm.status(status.current, status.total, status.done)?;
                        if status.done {
                            break;
                        }
                    }
                    Msg::Error(err) => {
                        log::error!("{}", err.err);
                        return Err(Error::Wipe(err.err));
                    }
                    _ => {
                        return Err(Error::Wipe("fs2dev err while wiping".into()));
                    }
                }
            }
        }

        comm.finalcopystatusdone(proto::usbsas::ResponseFinalCopyStatusDone {})?;

        let dev_size = children
            .fs2dev
            .comm
            .devsize(proto::fs2dev::RequestDevSize {})?
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
                Msg::Status(status) => {
                    comm.status(status.current, status.total, status.done)?;
                    if status.done {
                        comm.wipe(proto::usbsas::ResponseWipe {})?;
                        break;
                    }
                }
                _ => {
                    error!("bad response");
                    comm.error("bad response received from fs2dev")?;
                    break;
                }
            }
        }
        info!("wipe done");
        Ok(State::WaitEnd(WaitEndState {}))
    }
}

struct ImgDiskState {
    device: UsbMS,
}

impl ImgDiskState {
    fn run(self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
        info!("starting image disk: {}", self.device);
        self.image_disk(comm, children)?;
        comm.imgdisk(proto::usbsas::ResponseImgDisk {})?;
        Ok(State::WaitEnd(WaitEndState {}))
    }

    fn image_disk(&self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<()> {
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
            comm.status(
                offset * self.device.sector_size as u64,
                self.device.dev_size,
                false,
            )?;
        }
        info!("image disk done");
        Ok(())
    }
}

struct TransferDoneState {}

impl TransferDoneState {
    fn run(self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
        let req: proto::usbsas::Request = comm.recv()?;
        match req.msg.ok_or(Error::BadRequest)? {
            Msg::End(_) => {
                children.end_wait_all(comm)?;
                return Ok(State::End);
            }
            Msg::PostCopyCmd(req) => {
                info!("starting post copy cmd");
                match children
                    .cmdexec
                    .comm
                    .postcopyexec(proto::cmdexec::RequestPostCopyExec {
                        outfiletype: req.outfiletype,
                    }) {
                    Ok(_) => {
                        info!("post copy cmd done");
                        comm.postcopycmd(proto::usbsas::ResponsePostCopyCmd {})?;
                    }
                    Err(err) => {
                        error!("post copy cmd error: {}", err);
                        comm.error(err)?;
                    }
                }
            }
            _ => {
                error!("bad req");
                comm.error("bad request")?;
            }
        }
        Ok(State::WaitEnd(WaitEndState {}))
    }
}

struct WaitEndState {}

impl WaitEndState {
    fn run(self, comm: &mut ComRpUsbsas, children: &mut Children) -> Result<State> {
        loop {
            let req: proto::usbsas::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
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
        Ok(State::End)
    }
}

fn init_report() -> Result<serde_json::Value> {
    #[cfg(not(feature = "integration-tests"))]
    let (hostname, time) = {
        let name = match uname::Info::new() {
            Ok(name) => name.nodename,
            _ => "unknown-usbsas".to_string(),
        };
        (name, time::OffsetDateTime::now_utc())
    };
    // Fixed values to keep a deterministic filesystem hash
    #[cfg(feature = "integration-tests")]
    let (hostname, time) = (
        "unknown-usbsas",
        time::macros::datetime!(2020-01-01 0:00 UTC),
    );

    let report = json!({
        "title": format!("usbsas_transfer_{}", time),
        "timestamp": time.unix_timestamp(),
        "datetime": format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            time.year(), time.month() as u8, time.day(),
            time.hour(),time.minute(),time.second()),
        "hostname": hostname,
        "transfer_id": env::var("USBSAS_SESSION_ID").unwrap_or("0".to_string()),
    });
    Ok(report)
}

struct Children {
    analyzer: UsbsasChild<ComRqAnalyzer>,
    identificator: UsbsasChild<ComRqIdentificator>,
    cmdexec: UsbsasChild<ComRqCmdExec>,
    downloader: UsbsasChild<ComRqDownloader>,
    files2fs: UsbsasChild<ComRqWriteFs>,
    files2tar: UsbsasChild<ComRqWriteTar>,
    files2cleantar: UsbsasChild<ComRqWriteTar>,
    filter: UsbsasChild<ComRqFilter>,
    fs2dev: UsbsasChild<ComRqFs2Dev>,
    scsi2files: UsbsasChild<ComRqFiles>,
    tar2files: UsbsasChild<ComRqFiles>,
    uploader: UsbsasChild<ComRqUploader>,
    usbdev: UsbsasChild<ComRqUsbDev>,
}

// Functions shared by multiple states are implementend on this struct.
impl Children {
    fn id(&mut self, comm: &mut ComRpUsbsas, id: &mut Option<String>) -> Result<()> {
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

    // If destination is USB, check that device will have enough space to store
    // src files.
    // Returns max size of a single file (4GB if dest is FAT, None otherwise)
    fn check_dst_size(
        &mut self,
        comm: &mut ComRpUsbsas,
        destination: &Destination,
        total_files_size: u64,
    ) -> Result<Option<u64>> {
        // max_file_size is 4GB if we're writing a FAT fs, None otherwise
        match destination {
            Destination::Usb(ref usb) => {
                // Unlock fs2dev to get dev_size
                self.fs2dev.unlock_with(
                    &(((u64::from(usb.devnum)) << 32) | (u64::from(usb.busnum))).to_ne_bytes(),
                )?;
                let dev_size = self
                    .fs2dev
                    .comm
                    .devsize(proto::fs2dev::RequestDevSize {})?
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
                match OutFsType::try_from(usb.fstype) {
                    Ok(OutFsType::Fat) => Ok(Some(0xFFFF_FFFF)),
                    _ => Ok(None),
                }
            }
            Destination::Net(_) | Destination::Cmd(_) => Ok(None),
        }
    }

    fn end_all(&mut self) -> Result<()> {
        trace!("req end");
        if let Err(err) = self.analyzer.comm.end() {
            error!("Couldn't end analyzer: {}", err);
        };
        if let Err(err) = self.identificator.comm.end() {
            error!("Couldn't end identificator: {}", err);
        };
        self.cmdexec.unlock_with(&[0]).ok();
        if let Err(err) = self.cmdexec.comm.end() {
            error!("Couldn't end cmdexec: {}", err);
        };
        if let Err(err) = self.downloader.comm.end() {
            error!("Couldn't end downloader: {}", err);
        };
        if let Err(err) = self.files2fs.comm.end() {
            error!("Couldn't end files2fs: {}", err);
        };
        if let Err(err) = self.files2tar.comm.end() {
            error!("Couldn't end files2tar: {}", err);
        };
        if let Err(err) = self.files2cleantar.comm.end() {
            error!("Couldn't end files2cleantar: {}", err);
        };
        if let Err(err) = self.filter.comm.end() {
            error!("Couldn't end filter: {}", err);
        };
        self.fs2dev.unlock_with(&(0_u64).to_ne_bytes()).ok();
        if let Err(err) = self.fs2dev.comm.end() {
            error!("Couldn't end fs2dev: {}", err);
        };
        if let Err(err) = self.scsi2files.comm.end() {
            error!("Couldn't end scsi2files: {}", err);
        };
        self.tar2files.unlock_with(&[0]).ok();
        if let Err(err) = self.tar2files.comm.end() {
            error!("Couldn't end tar2files: {}", err);
        };
        self.uploader.unlock_with(&[0]).ok();
        if let Err(err) = self.uploader.comm.end() {
            error!("Couldn't end uploader: {}", err);
        };
        if let Err(err) = self.usbdev.comm.end() {
            error!("Couldn't end usbdev: {}", err);
        };
        Ok(())
    }

    fn wait_all(&mut self) -> Result<()> {
        debug!("waiting children");
        trace!("waiting analyzer");
        if let Err(err) = self.analyzer.wait() {
            error!("Waiting analyzer failed: {}", err);
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
        if let Err(err) = self.files2cleantar.wait() {
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

    fn end_wait_all(&mut self, comm: &mut ComRpUsbsas) -> Result<()> {
        trace!("req end");
        self.end_all()?;
        self.wait_all()?;
        comm.end()?;
        Ok(())
    }
}

pub struct Usbsas {
    comm: ComRpUsbsas,
    children: Children,
    state: State,
}

impl Usbsas {
    fn new(
        comm: ComRpUsbsas,
        config: Config,
        config_path: &str,
        out_files: OutFiles,
    ) -> Result<Self> {
        trace!("init");
        let mut pipes_read = vec![];
        let mut pipes_write = vec![];

        pipes_read.push(comm.input_fd());
        pipes_write.push(comm.output_fd());

        let identificator =
            UsbsasChildSpawner::new("usbsas-identificator").spawn::<ComRqIdentificator>()?;
        pipes_read.push(identificator.comm.input_fd());
        pipes_write.push(identificator.comm.output_fd());

        let cmdexec = UsbsasChildSpawner::new("usbsas-cmdexec")
            .arg(&out_files.tar_path)
            .arg(&out_files.fs_path)
            .args(&["-c", config_path])
            .wait_on_startup()
            .spawn::<ComRqCmdExec>()?;
        pipes_read.push(cmdexec.comm.input_fd());
        pipes_write.push(cmdexec.comm.output_fd());

        let downloader = UsbsasChildSpawner::new("usbsas-downloader")
            .arg(&out_files.tar_path)
            .args(&["-c", config_path])
            .spawn::<ComRqDownloader>()?;
        pipes_read.push(downloader.comm.input_fd());
        pipes_write.push(downloader.comm.output_fd());

        let usbdev = UsbsasChildSpawner::new("usbsas-usbdev")
            .args(&["-c", config_path])
            .spawn::<ComRqUsbDev>()?;
        pipes_read.push(usbdev.comm.input_fd());
        pipes_write.push(usbdev.comm.output_fd());

        let scsi2files = UsbsasChildSpawner::new("usbsas-scsi2files").spawn::<ComRqFiles>()?;
        pipes_read.push(scsi2files.comm.input_fd());
        pipes_write.push(scsi2files.comm.output_fd());

        let files2tar = UsbsasChildSpawner::new("usbsas-files2tar")
            .arg(&out_files.tar_path)
            .spawn::<ComRqWriteTar>()?;
        pipes_read.push(files2tar.comm.input_fd());
        pipes_write.push(files2tar.comm.output_fd());

        let files2cleantar = UsbsasChildSpawner::new("usbsas-files2tar")
            .arg(&format!(
                "{}_clean.tar",
                &out_files.tar_path.trim_end_matches(".tar")
            ))
            .spawn::<ComRqWriteTar>()?;
        pipes_read.push(files2cleantar.comm.input_fd());
        pipes_write.push(files2cleantar.comm.output_fd());

        let files2fs = UsbsasChildSpawner::new("usbsas-files2fs")
            .arg(&out_files.fs_path)
            .spawn::<ComRqWriteFs>()?;
        pipes_read.push(files2fs.comm.input_fd());
        pipes_write.push(files2fs.comm.output_fd());

        let filter = UsbsasChildSpawner::new("usbsas-filter")
            .args(&["-c", config_path])
            .spawn::<ComRqFilter>()?;
        pipes_read.push(filter.comm.input_fd());
        pipes_write.push(filter.comm.output_fd());

        let fs2dev = UsbsasChildSpawner::new("usbsas-fs2dev")
            .arg(&out_files.fs_path)
            .wait_on_startup()
            .spawn::<ComRqFs2Dev>()?;
        pipes_read.push(fs2dev.comm.input_fd());
        pipes_write.push(fs2dev.comm.output_fd());

        let tar2files = UsbsasChildSpawner::new("usbsas-tar2files")
            .arg(&out_files.tar_path)
            .wait_on_startup()
            .spawn::<ComRqFiles>()?;
        pipes_read.push(tar2files.comm.input_fd());
        pipes_write.push(tar2files.comm.output_fd());

        let uploader = UsbsasChildSpawner::new("usbsas-uploader")
            .arg(&out_files.tar_path)
            .wait_on_startup()
            .spawn::<ComRqUploader>()?;
        pipes_read.push(uploader.comm.input_fd());
        pipes_write.push(uploader.comm.output_fd());

        let analyzer = UsbsasChildSpawner::new("usbsas-analyzer")
            .arg(&out_files.tar_path)
            .args(&["-c", config_path])
            .spawn::<ComRqAnalyzer>()?;
        pipes_read.push(analyzer.comm.input_fd());
        pipes_write.push(analyzer.comm.output_fd());

        trace!("enter seccomp");
        usbsas_sandbox::usbsas::seccomp(pipes_read, pipes_write)?;

        let children = Children {
            analyzer,
            identificator,
            cmdexec,
            downloader,
            files2fs,
            files2tar,
            files2cleantar,
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
            state: State::Init(InitState { config }),
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
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            }
        }
        Ok(())
    }
}

struct Config {
    analyze_usb: bool,
    analyze_net: bool,
    analyze_cmd: bool,
    write_report_dest: bool,
    dst_networks: Option<Vec<usbsas_config::Network>>,
    src_network: Option<usbsas_config::Network>,
    command: Option<usbsas_config::Command>,
}

struct OutFiles {
    pub tar_path: String,
    pub clean_tar_path: String,
    pub fs_path: String,
}

fn main() -> Result<()> {
    let session_id = match env::var("USBSAS_SESSION_ID") {
        Ok(id) => id,
        Err(_) => {
            let id = uuid::Uuid::new_v4().simple().to_string();
            env::set_var("USBSAS_SESSION_ID", &id);
            id
        }
    };
    usbsas_utils::log::init_logger();

    let matches = usbsas_utils::clap::new_usbsas_cmd("usbsas-usbsas")
        .add_config_arg()
        .get_matches();
    let config_path = matches.get_one::<String>("config").unwrap();

    let config = conf_parse(&conf_read(config_path)?)?;

    let mut conf = Config {
        analyze_usb: false,
        analyze_net: false,
        analyze_cmd: false,
        write_report_dest: false,
        dst_networks: config.networks,
        src_network: config.source_network,
        command: config.command,
    };
    if let Some(analyzer_conf) = config.analyzer {
        conf.analyze_usb = analyzer_conf.analyze_usb;
        conf.analyze_net = analyzer_conf.analyze_net;
        conf.analyze_cmd = analyzer_conf.analyze_cmd;
    }
    if let Some(report_conf) = &config.report {
        conf.write_report_dest = report_conf.write_dest;
    };

    let out_files = OutFiles {
        tar_path: format!(
            "{}/usbsas_{}.tar",
            &config.out_directory.trim_end_matches('/'),
            session_id,
        ),
        clean_tar_path: format!(
            "{}/usbsas_{}_clean.tar",
            &config.out_directory.trim_end_matches('/'),
            session_id,
        ),
        fs_path: format!(
            "{}/usbsas_{}.img",
            &config.out_directory.trim_end_matches('/'),
            session_id,
        ),
    };

    info!(
        "init ({}): {} {} {}",
        std::process::id(),
        &out_files.tar_path,
        &out_files.clean_tar_path,
        &out_files.fs_path
    );

    let _ = File::create(&out_files.tar_path)?;
    let _ = File::create(&out_files.clean_tar_path)?;
    let _ = File::create(&out_files.fs_path)?;

    Usbsas::new(Comm::from_env()?, conf, config_path, out_files)?
        .main_loop()
        .map(|_| log::debug!("exit"))
}
