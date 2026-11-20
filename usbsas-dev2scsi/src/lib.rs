//! dev2scsi process of usbsas. It is responsible for opening the input USB
//! device, reading sectors from it and parsing its partition table.
//!
//! States: `Init` (wait for unlock value, open device, sandbox) -> `DevOpened`
//! (`Partitions`, `ReadSectors`) -> `PartitionsListed` (`ReadSectors`) -> `End`.

use byteorder::{ByteOrder, LittleEndian};
use log::{debug, error, trace, warn};
use std::{convert::TryFrom, io::prelude::*, str};
use thiserror::Error;
use usbsas_comm::{ComRpScsi, ProtoRespCommon, ProtoRespScsi};
use usbsas_proto as proto;
use usbsas_proto::{common::PartitionInfo, scsi::request::Msg};
use usbsas_utils::MAX_PROTO_MESS_SIZE;
#[cfg(not(any(test, feature = "mock")))]
use {
    std::os::unix::io::AsRawFd,
    usbsas_comm::ToFd,
    usbsas_mass_storage::{self, MassStorage},
};
#[cfg(any(test, feature = "mock"))]
use {
    std::{env, fs::OpenOptions},
    usbsas_mock::mass_storage::MockMassStorage as MassStorage,
};

/// Errors of this process.
#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("sandbox error: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("partition error: {0}")]
    Partition(String),
    #[cfg(not(any(test, feature = "mock")))]
    #[error("mass storage: {0}")]
    MassStorage(#[from] usbsas_mass_storage::Error),
    #[error("var error: {0}")]
    VarError(#[from] std::env::VarError),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
/// Result type of this process.
pub type Result<T> = std::result::Result<T, Error>;

// Max we need to read for ext4 check (other fs need less)
/// Bytes to read at the start of a partition to detect its file system
const MAX_LEN_PART_HEADER: u64 = 0x488;
/// Offset of the ISO9660 volume descriptors in a partition
const ISO_DESCRIPTORS_OFFSET: u64 = 0x8000;
/// Bytes to read from `ISO_DESCRIPTORS_OFFSET` to detect an ISO9660 image
/// ('CD001' in the first or second descriptor)
const LEN_ISO_HEADER: u64 = 0x806;
/// Max bytes read in one request: the response must fit in a message (with
/// room for the protobuf framing of the response).
const MAX_READ_SIZE: u64 = MAX_PROTO_MESS_SIZE - 64;

/// States of the process, see the module documentation.
enum State {
    Init(InitState),
    DevOpened(DevOpenedState),
    PartitionsListed(PartitionsListedState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpScsi) -> Result<Self> {
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

/// Read `count` sectors at sector `offset`. Reads beyond the end of the device
/// or whose response wouldn't fit in a message are refused before reading (or
/// allocating) anything.
fn read_sectors(ums: &mut MassStorage, offset: u64, count: u64) -> Result<Vec<u8>> {
    let block_size = u64::from(ums.block_size);
    if count
        .checked_mul(block_size)
        .is_none_or(|len| len > MAX_READ_SIZE)
    {
        return Err(Error::Error(format!("read of too many sectors: {count}")));
    }
    if offset
        .checked_add(count)
        .is_none_or(|end| end > ums.dev_size / block_size)
    {
        return Err(Error::Error(format!(
            "read out of device: {count} sectors at {offset}"
        )));
    }
    Ok(ums.read_sectors(offset, count, block_size as usize)?)
}

/// Answer a `ReadSectors` request, read errors are sent to the parent.
fn handle_read_sectors(
    ums: &mut MassStorage,
    comm: &mut ComRpScsi,
    req: proto::scsi::RequestReadSectors,
) -> Result<()> {
    match read_sectors(ums, req.offset, req.count) {
        Ok(data) => comm.readsectors(proto::scsi::ResponseReadSectors { data })?,
        Err(err) => {
            error!("{err}");
            comm.error(err)?;
        }
    }
    Ok(())
}

/// Detect the file system of `part` from `data`, its first
/// `MAX_LEN_PART_HEADER` bytes, and read its label.
fn detect_fs(part: &mut PartitionInfo, data: &[u8]) {
    // FAT12 / FAT16
    if let Ok("FAT") = str::from_utf8(&data[0x36..0x39]) {
        part.type_str = "FAT".into();
        if let Ok(name) = str::from_utf8(&data[0x2B..0x36]) {
            part.name_str = name.into();
        };
        if part.ptype == 0 {
            part.ptype = 0x6;
        }
    }
    // FAT32
    else if let Ok("FAT") = str::from_utf8(&data[0x52..0x55]) {
        part.type_str = "FAT".into();
        if let Ok(name) = str::from_utf8(&data[0x47..0x52]) {
            part.name_str = name.into();
        };
        if part.ptype == 0 {
            part.ptype = 0xb;
        }
    }
    // EXFAT
    else if let Ok("EXF") = str::from_utf8(&data[0x3..0x6]) {
        part.type_str = "EXFAT".into();
        if part.ptype == 0 {
            part.ptype = 0x7;
        }
    }
    // NTFS
    else if let Ok("NTFS") = str::from_utf8(&data[0x3..0x7]) {
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
            if let Ok(name) = str::from_utf8(&data[1024 + 0x78..1024 + 0x88]) {
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
}

impl InitState {
    /// Wait for the bus and device numbers of the device (`0` to exit), open
    /// it, sandbox the process and send `ResponseOpenDevice`.
    fn run(self, comm: &mut ComRpScsi) -> Result<State> {
        let mut buf = vec![0u8; 8];
        comm.read_exact(&mut buf)?;

        let busnum = LittleEndian::read_u32(&buf[0..4]);
        let devnum = LittleEndian::read_u32(&buf[4..8]);

        debug!("unlocked, busnum: {busnum} devnum: {devnum}");

        // If the process is unlocked with 0-0, usbsas is resetting, go directly to the EndState
        if busnum == 0 && devnum == 0 {
            #[cfg(not(any(test, feature = "mock")))]
            usbsas_sandbox::dev2scsi::seccomp(comm.input_fd(), comm.output_fd(), None)?;
            return Ok(State::WaitEnd(WaitEndState {}));
        }

        #[cfg(not(any(test, feature = "mock")))]
        let (device_file, device_fd) = {
            let device_path = format!("/dev/bus/usb/{busnum:03}/{devnum:03}");
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(device_path)
            {
                Ok(file) => {
                    let file_fd = file.as_raw_fd();
                    (file, file_fd)
                }
                Err(err) => {
                    error!("Error opening device file: {err}");
                    comm.error(err)?;
                    return Ok(State::WaitEnd(WaitEndState {}));
                }
            }
        };

        #[cfg(any(test, feature = "mock"))]
        let device_file = OpenOptions::new()
            .read(true)
            .write(false)
            .open(env::var("USBSAS_MOCK_IN_DEV")?)?;

        let usb_mass_storage = match MassStorage::from_opened_file(device_file) {
            Ok(ums) => ums,
            Err(err) => {
                error!("Init mass storage error: {err}, waiting end");
                comm.error(err)?;
                return Ok(State::WaitEnd(WaitEndState {}));
            }
        };

        #[cfg(not(any(test, feature = "mock")))]
        usbsas_sandbox::dev2scsi::seccomp(comm.input_fd(), comm.output_fd(), Some(device_fd))?;

        comm.opendevice(proto::scsi::ResponseOpenDevice {
            block_size: u64::from(usb_mass_storage.block_size),
            dev_size: usb_mass_storage.dev_size,
        })?;

        Ok(State::DevOpened(DevOpenedState { usb_mass_storage }))
    }
}

impl DevOpenedState {
    fn run(mut self, comm: &mut ComRpScsi) -> Result<State> {
        loop {
            match comm.recv_req()? {
                Msg::Partitions(_) => match self.partitions(comm) {
                    Ok(_) => break,
                    Err(err) => {
                        error!("{err}");
                        comm.error(err)?;
                    }
                },
                Msg::ReadSectors(req) => {
                    handle_read_sectors(&mut self.usb_mass_storage, comm, req)?
                }
                Msg::End(_) => {
                    comm.end()?;
                    return Ok(State::End);
                }
            }
        }
        Ok(State::PartitionsListed(PartitionsListedState {
            usb_mass_storage: self.usb_mass_storage,
        }))
    }

    /// Whether the device is an ISO9660 image.
    fn is_iso9660(&mut self) -> bool {
        let block_size = u64::from(self.usb_mass_storage.block_size);
        let count = LEN_ISO_HEADER.div_ceil(block_size);
        let data = match read_sectors(
            &mut self.usb_mass_storage,
            ISO_DESCRIPTORS_OFFSET / block_size,
            count,
        ) {
            Ok(data) => data,
            Err(err) => {
                warn!("couldn't read iso9660 header: {err}");
                return false;
            }
        };
        const ISO_MAGIC: &[u8] = b"CD001";
        ISO_MAGIC == &data[0x1..0x6] || ISO_MAGIC == &data[0x801..0x806]
    }

    /// List partitions (MBR or GPT) and detect their file system and label. A
    /// device without partition table is a single partition. Returns an error
    /// if no supported file system is found.
    fn partitions(&mut self, comm: &mut ComRpScsi) -> Result<()> {
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
                                    warn!("Unsupported partition type: {type_code}");
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
                warn!("error listing partitions (maybe no mbr ?): {err}");
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

        // Try to find name and also fs type if no bootsector was found
        let sectors_to_read = MAX_LEN_PART_HEADER.div_ceil(block_size);
        for part in partitions.iter_mut() {
            // A partition that can't be read (e.g. beyond the end of the
            // device) is listed with an unknown file system
            match read_sectors(&mut self.usb_mass_storage, part.start, sectors_to_read) {
                Ok(data) => detect_fs(part, &data),
                Err(err) => warn!("couldn't read partition at sector {}: {err}", part.start),
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
            if self.is_iso9660() {
                partitions[0].type_str = "ISO9660".into();
                // use this ptype since there isn't any for iso and we
                // should never support this one
                partitions[0].ptype = 0xFF;
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
    fn run(mut self, comm: &mut ComRpScsi) -> Result<State> {
        loop {
            match comm.recv_req()? {
                Msg::ReadSectors(req) => {
                    handle_read_sectors(&mut self.usb_mass_storage, comm, req)?
                }
                Msg::End(_) => {
                    comm.end()?;
                    break;
                }
                Msg::Partitions(_) => {
                    error!("partitions already listed");
                    comm.error("unexpected request")?;
                }
            }
        }
        Ok(State::End)
    }
}

impl WaitEndState {
    fn run(self, comm: &mut ComRpScsi) -> Result<State> {
        trace!("wait end state");
        loop {
            match comm.recv_req()? {
                Msg::End(_) => {
                    comm.end()?;
                    break;
                }
                _ => {
                    error!("unexpected req");
                    comm.error("unexpected request")?;
                }
            }
        }
        Ok(State::End)
    }
}

/// Source device reader process.
pub struct Dev2Scsi {
    comm: ComRpScsi,
    state: State,
}

impl Dev2Scsi {
    /// New reader, the device is given at unlock.
    pub fn new(comm: ComRpScsi) -> Result<Self> {
        let state = State::Init(InitState {});
        Ok(Dev2Scsi { comm, state })
    }

    /// Run the process until it receives `End`. On error, the error is sent to
    /// the parent and the process waits for `End`.
    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {err}, waiting end");
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
