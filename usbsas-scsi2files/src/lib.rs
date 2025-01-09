//! scsi2files is a usbsas process that requests sectors to dev2scsi and parses
//! file systems from data received.

use log::{error, trace};
use std::{
    convert::TryFrom,
    io::Write,
    sync::{Arc, RwLock},
};
use thiserror::Error;
use usbsas_comm::{
    ComRpFiles, ComRqScsi, ProtoReqCommon, ProtoReqScsi, ProtoRespCommon, ProtoRespFiles, SendRecv,
    ToFd,
};
use usbsas_fsrw::{ext4fs, ff, iso9660fs, ntfs, FSRead};
use usbsas_mass_storage::MassStorageComm;
use usbsas_process::{UsbsasChild, UsbsasChildSpawner};
use usbsas_proto as proto;
use usbsas_proto::{common::PartitionInfo, files::request::Msg};
use usbsas_utils::READ_FILE_MAX_SIZE;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("partition error: {0}")]
    Partition(String),
    #[error("fsrw: {0}")]
    Fsrw(#[from] usbsas_fsrw::Error),
    #[error("mass storage: {0}")]
    MassStorage(#[from] usbsas_mass_storage::Error),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("process: {0}")]
    Process(#[from] usbsas_process::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
pub type Result<T> = std::result::Result<T, Error>;

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
    fn run(self, comm: &mut ComRpFiles) -> Result<Self> {
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
    fn run(self, comm_parent: &mut ComRpFiles) -> Result<State> {
        let dev2scsi = UsbsasChildSpawner::new("usbsas-dev2scsi")
            .wait_on_startup()
            .spawn::<ComRqScsi>()?;
        let UsbsasChild { comm, .. } = dev2scsi;

        usbsas_sandbox::scsi2files::seccomp(
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
    child_comm: Option<Arc<RwLock<ComRqScsi>>>,
}

impl ChildStartedState {
    fn run(mut self, comm: &mut ComRpFiles) -> Result<State> {
        match comm.recv_req()? {
            Msg::OpenDevice(req) => {
                if let Err(err) = self.opendevice(comm, req.busnum, req.devnum) {
                    error!("err open device: {}, waiting end", err);
                    comm.error(err)?;
                    return Ok(State::WaitEnd(WaitEndState {
                        child_comm: Some(self.usb_mass.comm.clone()),
                    }));
                }
            }
            Msg::End(_) => {
                // unlock and end dev2scsi
                self.usb_mass.comm()?.write_all(&(0u64.to_le_bytes()))?;
                self.usb_mass.comm()?.end()?;
                comm.end()?;
                return Ok(State::End);
            }
            _ => return Err(Error::BadRequest),
        }
        Ok(State::DevOpened(DevOpenedState {
            usb_mass: self.usb_mass,
        }))
    }

    fn opendevice(&mut self, comm: &mut ComRpFiles, busnum: u32, devnum: u32) -> Result<()> {
        trace!("req opendevice");
        let buf = (u64::from(devnum) << 32) | u64::from(busnum);
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
    fn run(self, comm: &mut ComRpFiles) -> Result<State> {
        loop {
            match comm.recv_req()? {
                Msg::Partitions(_) => {
                    let partitions_infos = self.partitions(comm)?;
                    return Ok(State::PartitionsListed(PartitionsListedState {
                        usb_mass: self.usb_mass,
                        partitions_infos,
                    }));
                }
                Msg::ReadSectors(req) => self.read_sectors(comm, req.offset, req.count)?,
                Msg::End(_) => {
                    self.usb_mass.comm()?.end()?;
                    comm.end()?;
                    break;
                }
                _ => {
                    return Err(Error::BadRequest);
                }
            };
        }
        Ok(State::End)
    }

    fn partitions(&self, comm: &mut ComRpFiles) -> Result<Vec<PartitionInfo>> {
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

    fn read_sectors(&self, comm: &mut ComRpFiles, offset: u64, count: u64) -> Result<()> {
        let rep = self
            .usb_mass
            .comm()?
            .readsectors(proto::scsi::RequestReadSectors { offset, count })?;
        comm.readsectors(proto::files::ResponseReadSectors { data: rep.data })?;
        Ok(())
    }
}

impl PartitionsListedState {
    fn run(self, comm: &mut ComRpFiles) -> Result<State> {
        match comm.recv_req()? {
            Msg::OpenPartition(req) => {
                // Keep comm in case of error so we can end dev2scsi properly
                let comm_bk = self.usb_mass.comm.clone();
                match self.open_partition(comm, req.index) {
                    Ok(fs) => Ok(State::PartitionOpened(PartitionOpenedState { fs })),
                    Err(err) => {
                        comm.error(err)?;
                        Ok(State::WaitEnd(WaitEndState {
                            child_comm: Some(comm_bk),
                        }))
                    }
                }
            }
            Msg::End(_) => {
                self.usb_mass.comm()?.end()?;
                comm.end()?;
                Ok(State::End)
            }
            _ => Err(Error::BadRequest),
        }
    }

    fn open_partition(
        mut self,
        comm: &mut ComRpFiles,
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
    fn run(mut self, comm: &mut ComRpFiles) -> Result<State> {
        loop {
            let res = match comm.recv_req()? {
                Msg::GetAttr(req) => self.getattr(comm, req.path),
                Msg::ReadDir(req) => self.readdir(comm, req.path),
                Msg::ReadFile(req) => self.readfile(comm, req.path, req.offset, req.size),
                Msg::End(_) => break,
                _ => Err(Error::BadRequest),
            };
            if let Err(err) = res {
                comm.error(err)?;
            };
        }
        let usb_mass = self.fs.unmount_fs()?;
        usb_mass.comm()?.end()?;
        comm.end()?;
        Ok(State::End)
    }

    fn getattr(&mut self, comm: &mut ComRpFiles, path: String) -> Result<()> {
        trace!("req getattr {}", path);
        let (ftype, size, timestamp) = self.fs.get_attr(&path)?;
        comm.getattr(proto::files::ResponseGetAttr {
            ftype: ftype.into(),
            size,
            timestamp,
        })?;
        Ok(())
    }

    fn readdir(&mut self, comm: &mut ComRpFiles, path: String) -> Result<()> {
        trace!("req readdir {}", path);
        comm.readdir(proto::files::ResponseReadDir {
            filesinfo: self.fs.read_dir(&path)?,
        })?;
        Ok(())
    }

    fn readfile(
        &mut self,
        comm: &mut ComRpFiles,
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
    fn run(self, comm: &mut ComRpFiles) -> Result<State> {
        trace!("wait end state");
        loop {
            match comm.recv_req()? {
                Msg::End(_) => {
                    if let Some(child_comm) = self.child_comm {
                        if let Ok(mut child_comm) = child_comm.write() {
                            child_comm.end()?;
                        }
                    }
                    comm.end()?;
                    break;
                }
                _ => {
                    error!("bad request");
                    comm.error("bad request")?;
                }
            }
        }
        Ok(State::End)
    }
}

pub struct Scsi2Files {
    comm: ComRpFiles,
    state: State,
}

impl Scsi2Files {
    pub fn new(comm: ComRpFiles) -> Result<Self> {
        let state = State::Init(InitState {});
        Ok(Scsi2Files { comm, state })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}, waiting end", err);
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState { child_comm: None })
                }
            }
        }
        Ok(())
    }
}
