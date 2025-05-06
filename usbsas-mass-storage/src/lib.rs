//! Mass storage structs used by usbsas processes.

use positioned_io2::ReadAt;
use rusb::{Direction, GlobalContext, TransferType, UsbContext};
use std::{
    fs::File,
    io::{self, ErrorKind, Read, Seek, SeekFrom},
    os::unix::io::AsRawFd,
    sync::{Arc, RwLock},
    time::Duration,
};
use thiserror::Error;
use usbsas_comm::{ComRqScsi, ProtoReqScsi};
use usbsas_proto as proto;
use usbsas_scsi::ScsiUsb;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("rusb error: {0}")]
    Rusb(#[from] rusb::Error),
    #[error("{0}")]
    Error(String),
}
pub type Result<T> = std::result::Result<T, Error>;

pub const MAX_SECTORS_COUNT_CACHE: u64 = 8;
const MSC_SUBCLASS_RBC: u8 = 0x1; // Reduced Block Commands
const MSC_SUBCLASS_MMC5: u8 = 0x2; // Multi-Media Commands (CD/DVD)
const MSC_SUBCLASS_SCSI_TR: u8 = 0x6; // SCSI transparent command set
const MSC_PROTOCOL_CLASS_BULK_ONLY: u8 = 0x50;

// Mass storage struct used by dev2scsi
pub struct MassStorage {
    scsiusb: RwLock<ScsiUsb<GlobalContext>>,
    pub max_lba: u32,
    pub block_size: u32,
    pub dev_size: u64,
    pub pos: u64,
    _inner: Option<File>,
}

impl MassStorage {
    fn new(scsiusb: ScsiUsb<GlobalContext>, file: Option<File>) -> Result<Self> {
        let mut scsiusb = scsiusb;
        let (max_lba, block_size, dev_size) = scsiusb.init_mass_storage()?;
        // TODO: support more sector size
        assert!([0x200, 0x800, 0x1000].contains(&block_size));
        Ok(MassStorage {
            scsiusb: RwLock::new(scsiusb),
            max_lba,
            block_size,
            dev_size,
            pos: 0,
            _inner: file,
        })
    }

    pub fn from_opened_file(file: File) -> Result<Self> {
        rusb::disable_device_discovery()?;
        assert!(rusb::supports_detach_kernel_driver());

        let context = rusb::GlobalContext::default();
        let handle = unsafe { context.open_device_with_fd(file.as_raw_fd())? };
        let mut endpoints: [Option<u8>; 2] = [None; 2];
        for interface in handle.device().active_config_descriptor()?.interfaces() {
            for desc in interface.descriptors() {
                if desc.class_code() == rusb::constants::LIBUSB_CLASS_MASS_STORAGE
                    && (desc.sub_class_code() == MSC_SUBCLASS_RBC
                        || desc.sub_class_code() == MSC_SUBCLASS_MMC5
                        || desc.sub_class_code() == MSC_SUBCLASS_SCSI_TR)
                    && desc.protocol_code() == MSC_PROTOCOL_CLASS_BULK_ONLY
                {
                    for endp in desc.endpoint_descriptors() {
                        if endp.transfer_type() == TransferType::Bulk {
                            if endp.direction() == Direction::In {
                                endpoints[0] = Some(endp.address());
                            }
                            if endp.direction() == Direction::Out {
                                endpoints[1] = Some(endp.address());
                            }
                        }
                    }

                    if let [Some(ep0), Some(ep1)] = endpoints {
                        handle.set_auto_detach_kernel_driver(true)?;
                        handle.claim_interface(interface.number())?;
                        let scsiusb = ScsiUsb::new(
                            handle,
                            interface.number(),
                            desc.setting_number(),
                            ep0,
                            ep1,
                            Duration::from_secs(5),
                        );
                        return MassStorage::new(scsiusb, Some(file));
                    }
                }
            }
        }
        Err(Error::Error("Couldn't init libusb with opened file".into()))
    }

    pub fn read_sectors(&mut self, offset: u64, count: u64, block_size: usize) -> Result<Vec<u8>> {
        Ok(self
            .scsiusb
            .write()
            .map_err(|err| Error::Error(format!("write lock error: {}", err)))?
            .read_sectors(offset, count, block_size)?)
    }

    pub fn scsi_write_10(&mut self, buffer: &mut [u8], offset: u64, count: u64) -> Result<u8> {
        let ret = self
            .scsiusb
            .write()
            .map_err(|err| io::Error::new(ErrorKind::Other, format!("lock error: {err}")))?
            .scsi_write_10(buffer, offset, count)?;
        // Read last sector of what we've just written and verify it's ok.
        // XXX TODO FIXME Apparently, some devices requires reads between writes
        // to avoid overwriting cache of previous write call. Read call will
        // wait for the cache to be written before returning.
        // Hint: Linux seems to do this, check its code.
        let mut buf_check = vec![0; self.block_size as usize];
        self.scsiusb
            .write()
            .map_err(|err| io::Error::new(ErrorKind::Other, format!("lock error: {err}")))?
            .scsi_read_10(&mut buf_check, offset + count - 1, 1)?;
        if buf_check != buffer[(buffer.len() - buf_check.len())..] {
            return Err(Error::Error("write check failed".into()));
        }
        Ok(ret)
    }
}

impl Read for MassStorage {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos % (self.block_size as u64) != 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Read on non sector aligned",
            ));
        }
        if (buf.len() % (self.block_size as usize)) != 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Read on non sector size",
            ));
        }
        let offset = self.pos / (self.block_size as u64);
        let sectors = (buf.len() / (self.block_size as usize)) as u64;
        let data = self
            .scsiusb
            .write()
            .map_err(|err| io::Error::new(ErrorKind::Other, format!("lock error: {err}")))?
            .read_sectors(offset, sectors, self.block_size as usize)?;

        self.pos += buf.len() as u64;
        for (i, c) in data.iter().enumerate() {
            buf[i] = *c;
        }
        Ok(buf.len())
    }
}

impl Seek for MassStorage {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(pos) => {
                self.pos = pos;
            }
            _ => {
                return Err(io::Error::new(ErrorKind::InvalidInput, "Unsupported seek"));
            }
        }
        Ok(self.pos)
    }
}

impl ReadAt for MassStorage {
    fn read_at(&self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.read_exact_at(pos, buf)?;
        Ok(buf.len())
    }

    fn read_exact_at(&self, pos: u64, buf: &mut [u8]) -> io::Result<()> {
        if pos % (self.block_size as u64) != 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Read on non sector aligned",
            ));
        }

        if (buf.len() % (self.block_size as usize)) != 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Read on non sector size",
            ));
        }
        let offset = pos / (self.block_size as u64);
        let sectors = (buf.len() / (self.block_size as usize)) as u64;
        let data = self
            .scsiusb
            .write()
            .map_err(|err| io::Error::new(ErrorKind::Other, format!("lock error: {err}")))?
            .read_sectors(offset, sectors, self.block_size as usize)?;

        for (i, c) in data.iter().enumerate() {
            buf[i] = *c;
        }

        Ok(())
    }
}

// mass storage struct used by scsi2files
pub struct MassStorageComm {
    pub block_size: u32,
    pub seek: u64,
    pub dev_size: u64,
    pub partition_sector_start: u64,
    // RwLock because we need to impl ReadAt which takes a non mut ref
    pub comm: Arc<RwLock<ComRqScsi>>,
    // Small cache to avoid reading the same sectors multiple time
    pub cache: RwLock<lru::LruCache<(u64, u64), Vec<u8>>>,
}

impl MassStorageComm {
    pub fn new(comm: ComRqScsi) -> Self {
        MassStorageComm {
            block_size: 0,
            seek: 0,
            dev_size: 0,
            partition_sector_start: 0,
            comm: Arc::new(RwLock::new(comm)),
            cache: RwLock::new(lru::LruCache::new(
                // TODO: add an option to change this value in the configuration file
                // 32768 * 8 * 512 = 128MB (at most, count isn't always MAX_SECTORS_COUNT_CACHE)
                std::num::NonZeroUsize::new(32768).unwrap(),
            )),
        }
    }

    pub fn comm(&self) -> Result<std::sync::RwLockWriteGuard<'_, ComRqScsi>> {
        self.comm
            .write()
            .map_err(|err| Error::Error(format!("comm lock error: {err}")))
    }

    pub fn read_sectors(&self, offset: u64, count: u64) -> io::Result<Vec<u8>> {
        // Don't cache data if we're reading a lot of sectors,
        // it's probably a file (only read once) and not FS stuff
        if count <= MAX_SECTORS_COUNT_CACHE {
            if let Some(data) = self
                .cache
                .write()
                .map_err(|err| {
                    io::Error::new(ErrorKind::Other, format!("cache lock error: {err}"))
                })?
                .get(&(offset, count))
            {
                return Ok(data.clone());
            }
        }
        let rep = self
            .comm
            .write()
            .map_err(|err| io::Error::new(ErrorKind::Other, format!("comm lock error: {err}")))?
            .readsectors(proto::scsi::RequestReadSectors { offset, count })?;
        if count <= MAX_SECTORS_COUNT_CACHE {
            self.cache
                .write()
                .map_err(|err| {
                    io::Error::new(ErrorKind::Other, format!("cache lock error: {err}"))
                })?
                .put((offset, count), rep.data.clone());
        }
        Ok(rep.data)
    }
}

impl Read for MassStorageComm {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = buf.len() as u64;
        let block_size = self.block_size as u64;
        let read_offset = self.seek % block_size;
        let sectors_to_read = (read_offset + count).div_ceil(block_size);
        let offset = self.seek / block_size;

        let data = self.read_sectors(offset + self.partition_sector_start, sectors_to_read)?;

        self.seek += buf.len() as u64;

        let data = data[(read_offset as usize)..(read_offset as usize + count as usize)].to_vec();

        for (i, c) in data.iter().enumerate() {
            buf[i] = *c;
        }
        Ok(buf.len())
    }
}

impl Seek for MassStorageComm {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(pos) => {
                self.seek = pos;
            }
            SeekFrom::Current(pos) => match self.seek.checked_add(pos as u64) {
                Some(result) => {
                    self.seek = result;
                }
                _ => {
                    return Err(io::Error::new(ErrorKind::InvalidInput, "Unsupported seek"));
                }
            },
            _ => {
                return Err(io::Error::new(ErrorKind::InvalidInput, "Unsupported seek"));
            }
        }
        Ok(self.seek)
    }
}

// Needed for ext4-rs
impl ReadAt for MassStorageComm {
    fn read_at(&self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.read_exact_at(pos, buf)?;
        Ok(buf.len())
    }

    fn read_exact_at(&self, pos: u64, buf: &mut [u8]) -> io::Result<()> {
        let count = buf.len() as u64;
        let block_size = self.block_size as u64;
        let read_offset = pos % block_size;
        let sectors_to_read = (read_offset + count).div_ceil(block_size);
        let offset = pos / block_size;
        let data = self.read_sectors(offset + self.partition_sector_start, sectors_to_read)?;
        let data = data[(read_offset as usize)..(read_offset as usize + count as usize)].to_vec();
        for (i, c) in data.iter().enumerate() {
            buf[i] = *c;
        }
        Ok(())
    }
}
