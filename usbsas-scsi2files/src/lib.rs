//! scsi2files is a usbsas process that requests sectors to dev2scsi and parses
//! file systems from data received.

use log::{error, trace};
use std::{
    convert::TryFrom,
    io::Write,
    os::unix::io::RawFd,
    sync::{Arc, RwLock},
};
use thiserror::Error;
use usbsas_comm::{protorequest, protoresponse, Comm};
use usbsas_fsrw::{ext4fs, ff, iso9660fs, ntfs, FSRead};
use usbsas_mass_storage::MassStorageComm;
use usbsas_process::{UsbsasChild, UsbsasChildSpawner, UsbsasProcess};
use usbsas_proto as proto;
use usbsas_proto::{common::PartitionInfo, files::request::Msg};
use usbsas_utils::READ_FILE_MAX_SIZE;

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("partition error: {0}")]
    Partition(String),
    #[error("privileges: {0}")]
    Fsrw(#[from] usbsas_fsrw::Error),
    #[error("privileges: {0}")]
    Privileges(#[from] usbsas_privileges::Error),
    #[error("privileges: {0}")]
    Process(#[from] usbsas_process::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
type Result<T> = std::result::Result<T, Error>;

protoresponse!(
    CommFiles,
    files,
    opendevice = OpenDevice[ResponseOpenDevice],
    partitions = Partitions[ResponsePartitions],
    openpartition = OpenPartition[ResponseOpenPartition],
    getattr = GetAttr[ResponseGetAttr],
    readdir = ReadDir[ResponseReadDir],
    readfile = ReadFile[ResponseReadFile],
    readsectors = ReadSectors[ResponseReadSectors],
    error = Error[ResponseError],
    end = End[ResponseEnd]
);

protorequest!(
    CommScsi,
    scsi,
    partitions = Partitions[RequestPartitions, ResponsePartitions],
    readsectors = ReadSectors[RequestReadSectors, ResponseReadSectors],
    end = End[RequestEnd, ResponseEnd],
    opendev = OpenDevice[RequestOpenDevice, ResponseOpenDevice]
);

enum State {
    Init(InitState),
    ChildStarted(ChildStartedState),
    DevOpened(DevOpenedState),
    PartitionsListed(PartitionsListedState),
    PartitionOpened(PartitionOpenedState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut Comm<proto::files::Request>) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::ChildStarted(s) => s.run(comm),
            State::DevOpened(s) => s.run(comm),
            State::PartitionsListed(s) => s.run(comm),
            State::PartitionOpened(s) => s.run(comm),
            State::WaitEnd(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {}

impl InitState {
    fn run(self, comm_parent: &mut Comm<proto::files::Request>) -> Result<State> {
        let dev2scsi = UsbsasChildSpawner::new()
            .wait_on_startup()
            .spawn::<usbsas_dev2scsi::Dev2Scsi, proto::scsi::Request>()?;
        let UsbsasChild { comm, .. } = dev2scsi;

        usbsas_privileges::scsi2files::drop_priv(
            vec![comm_parent.input_fd(), comm.input_fd()],
            vec![comm_parent.output_fd(), comm.output_fd()],
        )?;

        let usb_mass = MassStorageComm::new(comm);

        Ok(State::ChildStarted(ChildStartedState { usb_mass }))
    }
}

struct ChildStartedState {
    usb_mass: MassStorageComm,
}

struct DevOpenedState {
    usb_mass: MassStorageComm,
}

struct PartitionsListedState {
    usb_mass: MassStorageComm,
    partitions_infos: Vec<PartitionInfo>,
}

struct PartitionOpenedState {
    fs: Box<dyn FSRead<MassStorageComm>>,
}

struct WaitEndState {
    child_comm: Option<Arc<RwLock<Comm<proto::scsi::Request>>>>,
}

impl ChildStartedState {
    fn run(mut self, comm: &mut Comm<proto::files::Request>) -> Result<State> {
        let req: proto::files::Request = comm.recv()?;
        match req.msg.ok_or(Error::BadRequest)? {
            Msg::OpenDevice(req) => {
                if let Err(err) = self.opendevice(comm, req.busnum, req.devnum) {
                    error!("err open device: {}, waiting end", err);
                    comm.error(proto::files::ResponseError {
                        err: format!("{}", err),
                    })?;
                    return Ok(State::WaitEnd(WaitEndState {
                        child_comm: Some(self.usb_mass.comm.clone()),
                    }));
                }
            }
            Msg::End(_) => {
                // unlock and end dev2scsi
                self.usb_mass.comm()?.write_all(&(0u64.to_le_bytes()))?;
                self.usb_mass.comm()?.end(proto::scsi::RequestEnd {})?;
                comm.end(proto::files::ResponseEnd {})?;
                return Ok(State::End);
            }
            _ => return Err(Error::BadRequest),
        }
        Ok(State::DevOpened(DevOpenedState {
            usb_mass: self.usb_mass,
        }))
    }

    fn opendevice(
        &mut self,
        comm: &mut Comm<proto::files::Request>,
        busnum: u32,
        devnum: u32,
    ) -> Result<()> {
        trace!("req opendevice");
        let buf = (u64::from(devnum)) << 32 | u64::from(busnum);
        // unlock dev2scsi
        self.usb_mass.comm()?.write_all(&buf.to_le_bytes())?;
        let rep: proto::scsi::Response = self.usb_mass.comm()?.recv()?;
        match rep.msg.ok_or(Error::BadRequest)? {
            proto::scsi::response::Msg::OpenDevice(rep) => {
                self.usb_mass.block_size = u32::try_from(rep.block_size)?;
                self.usb_mass.dev_size = rep.dev_size;
                comm.opendevice(proto::files::ResponseOpenDevice {
                    block_size: rep.block_size,
                    dev_size: rep.dev_size,
                })?;
            }
            proto::scsi::response::Msg::Error(rep) => return Err(Error::Error(rep.err)),
            _ => return Err(Error::BadRequest),
        }
        Ok(())
    }
}

impl DevOpenedState {
    fn run(self, comm: &mut Comm<proto::files::Request>) -> Result<State> {
        loop {
            let req: proto::files::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::Partitions(_) => {
                    let partitions_infos = self.partitions(comm)?;
                    return Ok(State::PartitionsListed(PartitionsListedState {
                        usb_mass: self.usb_mass,
                        partitions_infos,
                    }));
                }
                Msg::ReadSectors(req) => self.read_sectors(comm, req.offset, req.count)?,
                Msg::End(_) => {
                    self.usb_mass.comm()?.end(proto::scsi::RequestEnd {})?;
                    comm.end(proto::files::ResponseEnd {})?;
                    break;
                }
                _ => {
                    return Err(Error::BadRequest);
                }
            };
        }
        Ok(State::End)
    }

    fn partitions(&self, comm: &mut Comm<proto::files::Request>) -> Result<Vec<PartitionInfo>> {
        trace!("req partitions");
        let rep = self
            .usb_mass
            .comm()?
            .partitions(proto::scsi::RequestPartitions {})?;
        comm.partitions(proto::files::ResponsePartitions {
            partitions: rep.partitions.clone(),
        })?;
        Ok(rep.partitions)
    }

    fn read_sectors(
        &self,
        comm: &mut Comm<proto::files::Request>,
        offset: u64,
        count: u64,
    ) -> Result<()> {
        let rep = self
            .usb_mass
            .comm()?
            .readsectors(proto::scsi::RequestReadSectors { offset, count })?;
        comm.readsectors(proto::files::ResponseReadSectors { data: rep.data })?;
        Ok(())
    }
}

impl PartitionsListedState {
    fn run(self, comm: &mut Comm<proto::files::Request>) -> Result<State> {
        let req: proto::files::Request = comm.recv()?;
        match req.msg.ok_or(Error::BadRequest)? {
            Msg::OpenPartition(req) => {
                // Keep comm in case of error so we can end dev2scsi properly
                let comm_bk = self.usb_mass.comm.clone();
                match self.open_partition(comm, req.index) {
                    Ok(fs) => Ok(State::PartitionOpened(PartitionOpenedState { fs })),
                    Err(err) => {
                        comm.error(proto::files::ResponseError {
                            err: format!("{}", err),
                        })?;
                        Ok(State::WaitEnd(WaitEndState {
                            child_comm: Some(comm_bk),
                        }))
                    }
                }
            }
            Msg::End(_) => {
                self.usb_mass.comm()?.end(proto::scsi::RequestEnd {})?;
                comm.end(proto::files::ResponseEnd {})?;
                Ok(State::End)
            }
            _ => Err(Error::BadRequest),
        }
    }

    fn open_partition(
        mut self,
        comm: &mut Comm<proto::files::Request>,
        index: u32,
    ) -> Result<Box<dyn FSRead<MassStorageComm>>> {
        trace!("req open partition {}", index);
        let part_infos = if let Some(infos) = self.partitions_infos.get(index as usize) {
            infos
        } else {
            return Err(Error::Partition("Partition not found".into()));
        };
        log::info!("Reading partition: {:?}", part_infos);
        self.usb_mass.partition_sector_start = part_infos.start;
        let sector_size = self.usb_mass.block_size;
        let fs: Box<dyn FSRead<MassStorageComm>> = match part_infos.type_str.as_str() {
            "EXFAT" | "FAT" => Box::new(ff::FatFsReader::new(self.usb_mass, sector_size)?),
            "NTFS" => Box::new(ntfs::NTFS::new(self.usb_mass, sector_size)?),
            "Linux/Ext" => Box::new(ext4fs::Ext4::new(self.usb_mass, sector_size)?),
            "ISO9660" => Box::new(iso9660fs::Iso9660::new(self.usb_mass, sector_size)?),
            _ => return Err(Error::Partition("Unsupported filesystem".into())),
        };
        comm.openpartition(proto::files::ResponseOpenPartition {})?;
        Ok(fs)
    }
}

impl PartitionOpenedState {
    fn run(mut self, comm: &mut Comm<proto::files::Request>) -> Result<State> {
        loop {
            let req: proto::files::Request = comm.recv()?;
            let res = match req.msg.ok_or(Error::BadRequest)? {
                Msg::GetAttr(req) => self.getattr(comm, req.path),
                Msg::ReadDir(req) => self.readdir(comm, req.path),
                Msg::ReadFile(req) => self.readfile(comm, req.path, req.offset, req.size),
                Msg::End(_) => break,
                _ => Err(Error::BadRequest),
            };
            if let Err(err) = res {
                comm.error(proto::files::ResponseError {
                    err: format!("{}", err),
                })?;
            };
        }
        let usb_mass = self.fs.unmount_fs()?;
        usb_mass.comm()?.end(proto::scsi::RequestEnd {})?;
        comm.end(proto::files::ResponseEnd {})?;
        Ok(State::End)
    }

    fn getattr(&mut self, comm: &mut Comm<proto::files::Request>, path: String) -> Result<()> {
        trace!("req getattr {}", path);
        let (ftype, size, timestamp) = self.fs.get_attr(&path)?;
        comm.getattr(proto::files::ResponseGetAttr {
            ftype: ftype.into(),
            size,
            timestamp,
        })?;
        Ok(())
    }

    fn readdir(&mut self, comm: &mut Comm<proto::files::Request>, path: String) -> Result<()> {
        trace!("req readdir {}", path);
        comm.readdir(proto::files::ResponseReadDir {
            filesinfo: self.fs.read_dir(&path)?,
        })?;
        Ok(())
    }

    fn readfile(
        &mut self,
        comm: &mut Comm<proto::files::Request>,
        path: String,
        offset: u64,
        size: u64,
    ) -> Result<()> {
        let mut data = Box::new(vec![0u8; size as usize]);
        if size > READ_FILE_MAX_SIZE {
            return Err(Error::Error("max read size exceeded".to_string()));
        }
        self.fs.read_file(&path, &mut data, offset, size)?;
        comm.readfile(proto::files::ResponseReadFile { data: *data })?;
        Ok(())
    }
}

impl WaitEndState {
    fn run(self, comm: &mut Comm<proto::files::Request>) -> Result<State> {
        trace!("wait end state");
        loop {
            let req: proto::files::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::End(_) => {
                    if let Some(child_comm) = self.child_comm {
                        if let Ok(mut child_comm) = child_comm.write() {
                            child_comm.end(proto::scsi::RequestEnd {})?;
                        }
                    }
                    comm.end(proto::files::ResponseEnd {})?;
                    break;
                }
                _ => {
                    error!("bad request");
                    comm.error(proto::files::ResponseError {
                        err: "bad req, waiting end".into(),
                    })?;
                }
            }
        }
        Ok(State::End)
    }
}

pub struct Scsi2Files {
    comm: Comm<proto::files::Request>,
    state: State,
}

impl Scsi2Files {
    fn new(comm: Comm<proto::files::Request>) -> Result<Self> {
        let state = State::Init(InitState {});
        Ok(Scsi2Files { comm, state })
    }

    fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}, waiting end", err);
                    comm.error(proto::files::ResponseError {
                        err: format!("run error: {}", err),
                    })?;
                    State::WaitEnd(WaitEndState { child_comm: None })
                }
            }
        }
        Ok(())
    }
}

impl UsbsasProcess for Scsi2Files {
    fn spawn(
        read_fd: RawFd,
        write_fd: RawFd,
        _args: Option<Vec<String>>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        Scsi2Files::new(Comm::from_raw_fd(read_fd, write_fd))?
            .main_loop()
            .map(|_| log::debug!("scsi2files: exiting"))?;
        Ok(())
    }
}
