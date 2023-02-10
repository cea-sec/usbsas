//! dev2scsi process of usbsas. It is responsible for opening the input USB
//! device, reading sectors from it and parsing its partition table.
//!

use byteorder::{ByteOrder, LittleEndian};
use log::{debug, error, trace, warn};
use std::{convert::TryFrom, io::prelude::*, str};
use thiserror::Error;
use usbsas_comm::{protoresponse, Comm};
#[cfg(not(feature = "mock"))]
use usbsas_mass_storage::{self, MassStorage};
#[cfg(feature = "mock")]
use usbsas_mock::mass_storage::MockMassStorage as MassStorage;
use usbsas_proto as proto;
use usbsas_proto::{common::PartitionInfo, scsi::request::Msg};

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("rusb error: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("partition error: {0}")]
    Partition(String),
    #[cfg(not(feature = "mock"))]
    #[error("mass storage: {0}")]
    MassStorage(#[from] usbsas_mass_storage::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
pub type Result<T> = std::result::Result<T, Error>;

protoresponse!(
    CommScsi,
    scsi,
    partitions = Partitions[ResponsePartitions],
    readsectors = ReadSectors[ResponseReadSectors],
    end = End[ResponseEnd],
    opendev = OpenDevice[ResponseOpenDevice],
    error = Error[ResponseError]
);

// Max we need to read for ext4 check (other fs need less) and iso9660
const MAX_LEN_PART_HEADER: u64 = 0x464;
const MAX_LEN_ISO_HEADER: u64 = 0x8806;

enum State {
    Init(InitState),
    DevOpened(DevOpenedState),
    PartitionsListed(PartitionsListedState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut Comm<proto::scsi::Request>) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::DevOpened(s) => s.run(comm),
            State::PartitionsListed(s) => s.run(comm),
            State::WaitEnd(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {}

struct DevOpenedState {
    usb_mass_storage: MassStorage,
}

struct PartitionsListedState {
    usb_mass_storage: MassStorage,
}

struct WaitEndState {}

impl InitState {
    fn run(self, comm: &mut Comm<proto::scsi::Request>) -> Result<State> {
        let mut buf = vec![0u8; 8];
        comm.read_exact(&mut buf)?;

        let busnum = LittleEndian::read_u32(&buf[0..4]);
        let devnum = LittleEndian::read_u32(&buf[4..8]);

        debug!("unlocked, busnum: {} devnum: {}", busnum, devnum);

        // If the process is unlock with 0-0, usbsas is resetting, go directly
        // to the EndState
        if busnum == 0 && devnum == 0 {
            #[cfg(not(feature = "mock"))]
            usbsas_sandbox::dev2scsi::seccomp(
                comm.input_fd(),
                comm.output_fd(),
                usbsas_sandbox::get_libusb_opened_fds(busnum, devnum)?,
            )?;
            return Ok(State::WaitEnd(WaitEndState {}));
        }

        let usb_mass_storage = match MassStorage::from_busnum_devnum(busnum, devnum) {
            Ok(ums) => ums,
            Err(err) => {
                error!("Init mass storage error: {}, waiting end", err);
                comm.error(proto::scsi::ResponseError {
                    err: format!("{err}"),
                })?;
                return Ok(State::WaitEnd(WaitEndState {}));
            }
        };

        #[cfg(not(feature = "mock"))]
        usbsas_sandbox::dev2scsi::seccomp(
            comm.input_fd(),
            comm.output_fd(),
            usbsas_sandbox::get_libusb_opened_fds(busnum, devnum)?,
        )?;

        comm.opendev(proto::scsi::ResponseOpenDevice {
            block_size: u64::from(usb_mass_storage.block_size),
            dev_size: usb_mass_storage.dev_size,
        })?;

        Ok(State::DevOpened(DevOpenedState { usb_mass_storage }))
    }
}

impl DevOpenedState {
    fn run(mut self, comm: &mut Comm<proto::scsi::Request>) -> Result<State> {
        loop {
            let req: proto::scsi::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::Partitions(_) => match self.partitions(comm) {
                    Ok(_) => break,
                    Err(err) => {
                        error!("{}", err);
                        comm.error(proto::scsi::ResponseError {
                            err: format!("{err}"),
                        })?;
                    }
                },
                Msg::ReadSectors(req) => {
                    match self.usb_mass_storage.read_sectors(
                        req.offset,
                        req.count,
                        self.usb_mass_storage.block_size as usize,
                    ) {
                        Ok(data) => comm.readsectors(proto::scsi::ResponseReadSectors { data })?,
                        Err(err) => {
                            error!("{}", err);
                            comm.error(proto::scsi::ResponseError {
                                err: format!("{err}"),
                            })?;
                        }
                    }
                }
                Msg::End(_) => {
                    comm.end(proto::scsi::ResponseEnd {})?;
                    return Ok(State::End);
                }
                _ => {
                    error!("Unexpected req");
                    continue;
                }
            }
        }
        Ok(State::PartitionsListed(PartitionsListedState {
            usb_mass_storage: self.usb_mass_storage,
        }))
    }

    fn partitions(&mut self, comm: &mut Comm<proto::scsi::Request>) -> Result<()> {
        trace!("req partitions");
        let mut partitions = vec![];
        let block_size = self.usb_mass_storage.block_size as u64;
        let options = bootsector::Options {
            sector_size: bootsector::SectorSize::Known(u16::try_from(block_size)?),
            ..Default::default()
        };

        match bootsector::list_partitions(&mut self.usb_mass_storage, &options) {
            Ok(bootsec_parts) => {
                for part in bootsec_parts.iter() {
                    if part.first_byte % block_size != 0 {
                        return Err(Error::Partition("part start % block_size != 0".to_string()));
                    }
                    if part.len % block_size != 0 {
                        return Err(Error::Partition("part len % block_size != 0".to_string()));
                    }
                    match &part.attributes {
                        bootsector::Attributes::GPT { name, .. } => {
                            partitions.push(PartitionInfo {
                                ptype: 0,
                                start: part.first_byte / block_size,
                                size: part.len,
                                name_str: name.clone(),
                                type_str: "Unknown".into(),
                            });
                        }
                        bootsector::Attributes::MBR { type_code, .. } => {
                            let ptype = match type_code {
                                0x1 | // FAT12
                                0x4 | // FAT16 <32M
                                0x6 | // FAT16
                                0x7 | // NTFS / EXFAT
                                0xb | // W95 FAT32
                                0xc | // W95 FAT32 (LBA)
                                0xe | // W95 FAT16 (LBA)
                                0xf | // W95 Ext'd (LBA)
                                0x83  // Linux
                                    => *type_code as u32,
                                _ => {
                                    warn!("Unsupported partition type: {}", type_code);
                                    0
                                }
                            };
                            partitions.push(PartitionInfo {
                                ptype,
                                start: part.first_byte / block_size,
                                size: part.len,
                                name_str: "Unknown".into(),
                                type_str: "Unknown".into(),
                            });
                        }
                    }
                }
            }
            Err(err) => {
                warn!("error listing partitions (maybe no mbr ?): {}", err);
            }
        };

        // If bootsector didn't return any partition, a filesystem may be written directly on the
        // device without partition table, try to find it below
        if partitions.is_empty() {
            partitions.push(PartitionInfo {
                ptype: 0,
                start: 0,
                size: self.usb_mass_storage.dev_size,
                name_str: "Unknown".into(),
                type_str: "Unknown".into(),
            });
        }

        // Compute number of sectors to read to performs checks
        let mut sectors_to_read = MAX_LEN_PART_HEADER / block_size;
        if MAX_LEN_PART_HEADER.rem_euclid(block_size) > 0 {
            sectors_to_read += 1;
        }
        // Try to find name and also fs type if no bootsector was found
        for part in partitions.iter_mut() {
            let data = self.usb_mass_storage.read_sectors(
                part.start,
                sectors_to_read,
                block_size as usize,
            )?;

            // FAT12 / FAT16
            if let Ok("FAT") = str::from_utf8(data[0x36..0x39].into()) {
                part.type_str = "FAT".into();
                if let Ok(name) = str::from_utf8(data[0x26..0x31].into()) {
                    part.name_str = name.into();
                };
                if part.ptype == 0 {
                    part.ptype = 0x6;
                }
            }
            // FAT32
            else if let Ok("FAT") = str::from_utf8(data[0x52..0x55].into()) {
                part.type_str = "FAT".into();
                if let Ok(name) = str::from_utf8(data[0x47..0x52].into()) {
                    part.name_str = name.into();
                };
                if part.ptype == 0 {
                    part.ptype = 0xb;
                }
            }
            // EXFAT
            else if let Ok("EXF") = str::from_utf8(data[0x3..0x6].into()) {
                part.type_str = "EXFAT".into();
                if part.ptype == 0 {
                    part.ptype = 0x7;
                }
            }
            // NTFS
            else if let Ok("NTFS") = str::from_utf8(data[0x3..0x7].into()) {
                part.type_str = "NTFS".into();
                // Reading NTFS volume name requires to parse the fs, we're not
                // doing this here.
                if part.ptype == 0 {
                    part.ptype = 0x7;
                }
            }
            // Linux/Ext
            else if let [0x53, 0xEF] = data[0x438..0x43A] {
                // ext4 check (as the unix 'file' cmd does)
                if LittleEndian::read_u32(&data[0x460..0x464]) > 63 {
                    part.type_str = "Linux/Ext".into();
                    // Ext4 Volume Label
                    if let Ok(name) = str::from_utf8(data[1024 + 0x78..1024 + 0x88].into()) {
                        part.name_str = name.into();
                    };
                    if part.ptype == 0 {
                        part.ptype = 0x83;
                    }
                }
                // ext2 & 3 not supported
                else {
                    part.ptype = 0;
                }
            }
            // Trim 0 and leading / trailing whitespaces
            part.name_str = part.name_str.trim_end_matches(char::from(0)).trim().into();
            if part.name_str.is_empty() {
                part.name_str = "Unknown".into();
            }
        }

        // If we didn't find anything supported, last try with ISO9660 which requires different
        // sectors to read
        if partitions.len() == 1 && partitions[0].ptype == 0 {
            // Check for 'CD001' at 0x8001 and 0x8801
            sectors_to_read = MAX_LEN_ISO_HEADER / block_size;
            if MAX_LEN_ISO_HEADER.rem_euclid(block_size) > 0 {
                sectors_to_read += 1;
            }
            let data = self.usb_mass_storage.read_sectors(
                0x8000 / block_size,
                sectors_to_read,
                block_size as usize,
            )?;
            if [0x43, 0x44, 0x30, 0x30, 0x31] == data[0x1..0x6]
                || [0x43, 0x44, 0x30, 0x30, 0x31] == data[0x801..0x806]
            {
                partitions[0].type_str = "ISO9660".into();
                if partitions[0].ptype == 0 {
                    // use this ptype since there isn't any for iso and we
                    // should never support this one
                    partitions[0].ptype = 0xFF;
                }
            } else {
                return Err(Error::Partition(
                    "No supported filesystem found".to_string(),
                ));
            }
        }

        comm.partitions(proto::scsi::ResponsePartitions { partitions })?;
        Ok(())
    }
}

impl PartitionsListedState {
    fn run(mut self, comm: &mut Comm<proto::scsi::Request>) -> Result<State> {
        loop {
            let req: proto::scsi::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::ReadSectors(req) => {
                    match self.usb_mass_storage.read_sectors(
                        req.offset,
                        req.count,
                        self.usb_mass_storage.block_size as usize,
                    ) {
                        Ok(data) => comm.readsectors(proto::scsi::ResponseReadSectors { data })?,
                        Err(err) => {
                            error!("{}", err);
                            comm.error(proto::scsi::ResponseError {
                                err: format!("{err}"),
                            })?;
                        }
                    }
                }
                Msg::End(_) => {
                    comm.end(proto::scsi::ResponseEnd {})?;
                    break;
                }
                _ => {
                    error!("Unexpected req");
                    continue;
                }
            }
        }
        Ok(State::End)
    }
}

impl WaitEndState {
    fn run(self, comm: &mut Comm<proto::scsi::Request>) -> Result<State> {
        trace!("wait end state");
        let req: proto::scsi::Request = comm.recv()?;
        match req.msg.ok_or(Error::BadRequest)? {
            Msg::End(_) => {
                comm.end(proto::scsi::ResponseEnd {})?;
            }
            _ => {
                error!("unexpected req");
                comm.error(proto::scsi::ResponseError {
                    err: "bad request".into(),
                })?;
            }
        }
        Ok(State::End)
    }
}

pub struct Dev2Scsi {
    comm: Comm<proto::scsi::Request>,
    state: State,
}

impl Dev2Scsi {
    pub fn new(comm: Comm<proto::scsi::Request>) -> Result<Self> {
        let state = State::Init(InitState {});
        Ok(Dev2Scsi { comm, state })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}, waiting end", err);
                    comm.error(proto::scsi::ResponseError {
                        err: format!("run error: {err}"),
                    })?;
                    State::WaitEnd(WaitEndState {})
                }
            }
        }
        Ok(())
    }
}
