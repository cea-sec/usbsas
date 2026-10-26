//! SCSI over USB Mass Storage Bulk-Only Transport.
//!
//! [`ScsiUsb`] drives a USB mass storage device directly through rusb (libusb
//! rust bindings):
//!
//! - commands are wrapped in Command Block Wrappers (CBW) sent on the bulk out
//!   endpoint, data moves on the bulk endpoints and the device answers with a
//!   Command Status Wrapper (CSW),
//! - [`ScsiUsb::init_mass_storage`] selects a ready logical unit and reads its
//!   capacity, switching to 16-byte commands for large devices,
//! - blocks are then read and written with READ / WRITE (10 or 16).
//!
//! The USB handle is abstracted by [`UsbTransport`] so that the protocol can be
//! tested against a simulated device (see `tests/simulated_device.rs`).

mod bot;
mod cdb;
mod error;
mod sense;
mod transport;

use bot::{CBW_LEN, CSW_LEN, Cbw, Csw, CswStatus, Direction};
use cdb::Cdb;
use sense::{ASC_INVALID_FIELD_IN_CDB, ASC_INVALID_OPCODE, ASC_LUN_NOT_READY};

use log::{debug, error, warn};
use rusb::{Direction as UsbDirection, Recipient, RequestType, request_type};
use std::{thread, time::Duration};

pub use error::{Error, Result, Stage};
pub use sense::{SenseData, SenseKey};
pub use transport::UsbTransport;

/// Maximum number of blocks requested by a single command in [`ScsiUsb::read_sectors`].
pub const MAX_SECTORS_PER_TRANSFER: u16 = 0x800;

/// Attempts at reading a CSW when the device times out.
const CSW_READ_ATTEMPTS: u32 = 3;
/// TEST UNIT READY polls per logical unit before giving up on it.
const TEST_UNIT_READY_ATTEMPTS: u32 = 100;
/// Delay between polls while a unit is becoming ready.
const NOT_READY_DELAY: Duration = Duration::from_millis(200);

const INQUIRY_LEN: u8 = 36;
const SENSE_LEN: u8 = 18;
const READ_CAPACITY_10_LEN: usize = 8;
const READ_CAPACITY_16_LEN: usize = 32;
const READ_CAPACITY_16_MIN_LEN: usize = 12;

const PERIPHERAL_DIRECT_ACCESS: u8 = 0x00;
const PERIPHERAL_CD_DVD: u8 = 0x05;

/// Mass storage class requests.
const REQ_GET_MAX_LUN: u8 = 0xfe;
const REQ_BULK_ONLY_RESET: u8 = 0xff;
/// LUNs are 4 bits wide in a CBW.
const MAX_LUN: u8 = 15;

/// Fallback when the endpoint descriptor cannot be read (high-speed bulk size).
const DEFAULT_MAX_PACKET_SIZE: usize = 512;

/// Capacity reported by READ CAPACITY(10) or READ CAPACITY(16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capacity {
    /// Address of the last logical block.
    pub max_lba: u64,
    /// Block length in bytes.
    pub block_size: u32,
}

impl Capacity {
    /// Number of blocks of the device.
    pub fn block_count(&self) -> u64 {
        self.max_lba.saturating_add(1)
    }

    /// Device size in bytes.
    pub fn device_size(&self) -> u64 {
        self.block_count()
            .saturating_mul(u64::from(self.block_size))
    }

    fn validated(self) -> Result<Self> {
        if self.block_size == 0 || self.block_size > u32::from(u16::MAX) {
            return Err(Error::InvalidBlockSize(self.block_size.into()));
        }
        Ok(self)
    }
}

/// Which 16-byte commands a device accepts, as found by
/// [`ScsiUsb::detect_16_byte_support`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Support16 {
    /// READ CAPACITY(16) is accepted
    pub read_capacity_16: bool,
    /// READ(16) is accepted
    pub read_16: bool,
    /// WRITE(16) is accepted
    pub write_16: bool,
}

impl Support16 {
    /// Whether the device can be driven with 16-byte commands only.
    pub fn all(&self) -> bool {
        self.read_capacity_16 && self.read_16 && self.write_16
    }
}

/// Data stage of a Bulk-Only command.
enum Data<'a> {
    None,
    In(&'a mut [u8]),
    Out(&'a [u8]),
}

/// Result of a command that completed at the transport level.
#[derive(Debug, Clone, Copy)]
struct Outcome {
    passed: bool,
    /// Bytes actually moved during the data stage.
    transferred: usize,
}

/// A USB mass storage device speaking SCSI over the Bulk-Only Transport.
pub struct ScsiUsb<H: UsbTransport> {
    handle: H,
    interface: u8,
    endpoint_in: u8,
    endpoint_out: u8,
    max_packet_size: usize,
    timeout: Duration,
    tag: u32,
    lun: u8,
    use_16: bool,
}

impl<H: UsbTransport> ScsiUsb<H> {
    /// Wrap a handle whose mass storage `interface` is already claimed.
    ///
    /// Selects the alternate setting `interface_alt`; a failure there is only
    /// logged, since many single-setting devices reject the request.
    pub fn new(
        handle: H,
        interface: u8,
        interface_alt: u8,
        endpoint_in: u8,
        endpoint_out: u8,
        timeout: Duration,
    ) -> Self {
        if let Err(err) = handle.set_alternate_setting(interface, interface_alt) {
            warn!(
                "couldn't select alternate setting {interface_alt} of interface {interface}: {err}"
            );
        }
        let max_packet_size = handle
            .max_packet_size(endpoint_in)
            .filter(|&size| size != 0)
            .map_or(DEFAULT_MAX_PACKET_SIZE, usize::from);
        Self {
            handle,
            interface,
            endpoint_in,
            endpoint_out,
            max_packet_size,
            timeout,
            tag: 0,
            lun: 0,
            use_16: false,
        }
    }

    /// Underlying USB handle.
    pub fn handle(&self) -> &H {
        &self.handle
    }

    /// Logical unit commands are sent to.
    pub fn lun(&self) -> u8 {
        self.lun
    }

    /// Whether [`Self::read_blocks`], [`Self::write_blocks`] and
    /// [`Self::read_sectors`] use READ(16)/WRITE(16) rather than READ(10)/WRITE(10).
    pub fn uses_16_byte_commands(&self) -> bool {
        self.use_16
    }

    /// Choose the read/write commands, e.g. after
    /// [`Self::detect_16_byte_support`]. [`Self::init_mass_storage`] sets it
    /// automatically for devices that need it.
    pub fn set_use_16_byte_commands(&mut self, use_16: bool) {
        self.use_16 = use_16;
    }

    /// Issue a USB port reset of the device.
    pub fn reset_device(&mut self) -> Result<()> {
        self.handle.reset().map_err(Error::usb(Stage::Control))
    }

    /// Find the first ready direct-access or CD/DVD logical unit, select it
    /// and return its capacity.
    ///
    /// Switches to 16-byte read/write commands when the device is too large
    /// for READ CAPACITY(10) and answers READ CAPACITY(16).
    pub fn init_mass_storage(&mut self) -> Result<Capacity> {
        let max_lun = self.get_max_lun();
        debug!("init mass storage, max lun: {max_lun}");

        let mut candidates = Vec::new();
        for lun in 0..=max_lun {
            self.lun = lun;
            match self.inquiry()? {
                Some(byte0) => {
                    let qualifier = byte0 >> 5;
                    let device_type = byte0 & 0x1f;
                    debug!("lun {lun}: qualifier {qualifier}, type {device_type:#04x}");
                    if qualifier == 0
                        && matches!(device_type, PERIPHERAL_DIRECT_ACCESS | PERIPHERAL_CD_DVD)
                    {
                        candidates.push(lun);
                    }
                }
                None => warn!("lun {lun}: INQUIRY failed, skipping"),
            }
        }
        debug!("candidate luns: {candidates:?}");

        for lun in candidates {
            debug!("testing lun {lun}");
            self.lun = lun;
            self.inquiry()?;
            if self.wait_until_ready()? {
                return self.detect_capacity();
            }
        }

        error!("no ready lun found");
        Err(Error::NoReadyLun)
    }

    /// Read `count` blocks of `block_size` bytes starting at `offset`, split
    /// into commands of at most [`MAX_SECTORS_PER_TRANSFER`] blocks, with
    /// [`Self::read_blocks`].
    pub fn read_sectors(&mut self, offset: u64, count: u64, block_size: usize) -> Result<Vec<u8>> {
        if block_size == 0 {
            return Err(Error::InvalidBlockSize(0));
        }
        let len = usize::try_from(count)
            .ok()
            .and_then(|count| count.checked_mul(block_size))
            .ok_or(Error::OutOfRange {
                what: "sector count",
                value: count,
            })?;
        let mut buffer = vec![0; len];
        let chunk_len = block_size * usize::from(MAX_SECTORS_PER_TRANSFER);
        let mut lba = offset;
        for chunk in buffer.chunks_mut(chunk_len) {
            let blocks = (chunk.len() / block_size) as u64;
            self.read_blocks(chunk, lba, blocks)?;
            lba += blocks;
        }
        Ok(buffer)
    }

    /// Read `count` blocks at `offset` with READ(16) or READ(10), depending on
    /// [`Self::uses_16_byte_commands`].
    pub fn read_blocks(&mut self, buffer: &mut [u8], offset: u64, count: u64) -> Result<()> {
        if self.use_16 {
            self.scsi_read_16(buffer, offset, count)
        } else {
            self.scsi_read_10(buffer, offset, count)
        }
    }

    /// Write `count` blocks at `offset` with WRITE(16) or WRITE(10), depending
    /// on [`Self::uses_16_byte_commands`].
    pub fn write_blocks(&mut self, buffer: &[u8], offset: u64, count: u64) -> Result<()> {
        if self.use_16 {
            self.scsi_write_16(buffer, offset, count)
        } else {
            self.scsi_write_10(buffer, offset, count)
        }
    }

    /// READ(10) `count` blocks at `offset` into `buffer`, which must be
    /// exactly `count` blocks long.
    pub fn scsi_read_10(&mut self, buffer: &mut [u8], offset: u64, count: u64) -> Result<()> {
        let cdb = Cdb::read_10(lba_10(offset)?, blocks_10(count)?);
        let expected = buffer.len();
        let outcome = self.execute(&cdb, Data::In(buffer))?;
        self.ensure_complete(&cdb, outcome, expected, Stage::DataIn)
    }

    /// WRITE(10) `buffer`, which must be exactly `count` blocks long, at `offset`.
    pub fn scsi_write_10(&mut self, buffer: &[u8], offset: u64, count: u64) -> Result<()> {
        let cdb = Cdb::write_10(lba_10(offset)?, blocks_10(count)?);
        let outcome = self.execute(&cdb, Data::Out(buffer))?;
        self.ensure_complete(&cdb, outcome, buffer.len(), Stage::DataOut)
    }

    /// READ(16) `count` blocks at `offset` into `buffer`, which must be
    /// exactly `count` blocks long.
    pub fn scsi_read_16(&mut self, buffer: &mut [u8], offset: u64, count: u64) -> Result<()> {
        let cdb = Cdb::read_16(offset, blocks_16(count)?);
        let expected = buffer.len();
        let outcome = self.execute(&cdb, Data::In(buffer))?;
        self.ensure_complete(&cdb, outcome, expected, Stage::DataIn)
    }

    /// WRITE(16) `buffer`, which must be exactly `count` blocks long, at `offset`.
    pub fn scsi_write_16(&mut self, buffer: &[u8], offset: u64, count: u64) -> Result<()> {
        let cdb = Cdb::write_16(offset, blocks_16(count)?);
        let outcome = self.execute(&cdb, Data::Out(buffer))?;
        self.ensure_complete(&cdb, outcome, buffer.len(), Stage::DataOut)
    }

    /// READ CAPACITY(10) of the current LUN. A `max_lba` of `0xffffffff`
    /// means the device is too large and READ CAPACITY(16) is needed.
    pub fn read_capacity_10(&mut self) -> Result<Capacity> {
        let cdb = Cdb::read_capacity_10();
        let mut buf = [0; READ_CAPACITY_10_LEN];
        let outcome = self.execute(&cdb, Data::In(&mut buf))?;
        self.ensure_complete(&cdb, outcome, buf.len(), Stage::DataIn)?;
        let (max_lba, block_size) = buf.split_at(4);
        Capacity {
            max_lba: u32::from_be_bytes(max_lba.try_into()?).into(),
            block_size: u32::from_be_bytes(block_size.try_into()?),
        }
        .validated()
    }

    /// READ CAPACITY(16) of the current LUN.
    pub fn read_capacity_16(&mut self) -> Result<Capacity> {
        let cdb = Cdb::read_capacity_16(READ_CAPACITY_16_LEN as u32);
        let mut buf = [0; READ_CAPACITY_16_LEN];
        let outcome = self.execute(&cdb, Data::In(&mut buf))?;
        // Devices may return less than the allocation length; only the first
        // 12 bytes matter here.
        self.ensure_complete(&cdb, outcome, outcome.transferred, Stage::DataIn)?;
        if outcome.transferred < READ_CAPACITY_16_MIN_LEN {
            return Err(Error::ShortTransfer {
                stage: Stage::DataIn,
                expected: READ_CAPACITY_16_MIN_LEN,
                actual: outcome.transferred,
            });
        }
        Capacity {
            max_lba: u64::from_be_bytes(buf[0..8].try_into()?),
            block_size: u32::from_be_bytes(buf[8..12].try_into()?),
        }
        .validated()
    }

    /// Probe which of READ CAPACITY(16), READ(16) and WRITE(16) the current
    /// LUN implements. Call it after [`Self::init_mass_storage`].
    ///
    /// Nothing is read from or written to the medium: commands are sent with a
    /// transfer length of zero, which is considered a no-op.
    /// A command is unsupported when the device rejects it with ILLEGAL
    /// REQUEST (invalid opcode or invalid field in CDB), or answers with a
    /// phase error or garbage, after which the transport is reset.
    ///
    /// This only reports; use [`Self::set_use_16_byte_commands`] to act on it.
    pub fn detect_16_byte_support(&mut self) -> Result<Support16> {
        let mut buf = [0; READ_CAPACITY_16_LEN];
        let support = Support16 {
            read_capacity_16: self.probe(
                &Cdb::read_capacity_16(READ_CAPACITY_16_LEN as u32),
                Some(&mut buf),
                READ_CAPACITY_16_MIN_LEN,
            )?,
            read_16: self.probe(&Cdb::read_16(0, 0), None, 0)?,
            write_16: self.probe(&Cdb::write_16(0, 0), None, 0)?,
        };
        debug!("16-byte command support: {support:?}");
        Ok(support)
    }

    /// Send `cdb` and report whether the device implements it; see
    /// [`Self::detect_16_byte_support`]. A unit attention is retried once.
    fn probe(&mut self, cdb: &Cdb, mut buf: Option<&mut [u8]>, min_len: usize) -> Result<bool> {
        for _ in 0..2 {
            let data = match buf.as_deref_mut() {
                Some(buf) => Data::In(buf),
                None => Data::None,
            };
            let outcome = match self.execute(cdb, data) {
                Ok(outcome) => outcome,
                Err(Error::PhaseError | Error::InvalidCsw(_)) => {
                    debug!(
                        "command {:#04x}: transport error, unsupported",
                        cdb.opcode()
                    );
                    return Ok(false);
                }
                Err(err) => return Err(err),
            };
            if outcome.passed {
                return Ok(outcome.transferred >= min_len);
            }
            let Some(sense) = self.request_sense()? else {
                debug!("command {:#04x} failed without sense data", cdb.opcode());
                return Ok(false);
            };
            match sense.key {
                SenseKey::UnitAttention => continue,
                SenseKey::IllegalRequest
                    if matches!(sense.asc, ASC_INVALID_OPCODE | ASC_INVALID_FIELD_IN_CDB) =>
                {
                    return Ok(false);
                }
                // Any other failure (not ready, write protected...) means the
                // command was understood.
                _ => {
                    debug!(
                        "command {:#04x} recognised but failed: {sense}",
                        cdb.opcode()
                    );
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Poll the current LUN with TEST UNIT READY until it is ready (`true`)
    /// or reports a condition it won't recover from (`false`).
    fn wait_until_ready(&mut self) -> Result<bool> {
        for _ in 0..TEST_UNIT_READY_ATTEMPTS {
            if self.execute(&Cdb::test_unit_ready(), Data::None)?.passed {
                debug!("unit ready");
                return Ok(true);
            }

            let sense = match self.request_sense() {
                Ok(Some(sense)) => sense,
                Ok(None) => {
                    warn!("unit not ready, no usable sense data");
                    continue;
                }
                Err(err) => {
                    error!("REQUEST SENSE failed: {err}");
                    return Ok(false);
                }
            };
            debug!("unit not ready: {sense}");

            match sense.key {
                SenseKey::NoSense | SenseKey::RecoveredError | SenseKey::Completed => {}
                SenseKey::NotReady if sense.asc == ASC_LUN_NOT_READY => {
                    thread::sleep(NOT_READY_DELAY)
                }
                SenseKey::UnitAttention => thread::sleep(NOT_READY_DELAY),
                SenseKey::NotReady
                | SenseKey::MediumError
                | SenseKey::HardwareError
                | SenseKey::IllegalRequest => {
                    error!("unit failed: {sense}");
                    return Ok(false);
                }
                _ => error!("unhandled sense: {sense}"),
            }
        }
        warn!("unit still not ready after {TEST_UNIT_READY_ATTEMPTS} attempts");
        Ok(false)
    }

    /// INQUIRY the current LUN; returns the peripheral qualifier/type byte,
    /// or `None` if the command failed.
    fn inquiry(&mut self) -> Result<Option<u8>> {
        let mut buf = [0; INQUIRY_LEN as usize];
        let outcome = self.execute(&Cdb::inquiry(INQUIRY_LEN), Data::In(&mut buf))?;
        Ok((outcome.passed && outcome.transferred > 0).then_some(buf[0]))
    }

    /// REQUEST SENSE; returns `None` if the command failed or the data is unparsable.
    fn request_sense(&mut self) -> Result<Option<SenseData>> {
        let mut buf = [0; SENSE_LEN as usize];
        let outcome = self.execute(&Cdb::request_sense(SENSE_LEN), Data::In(&mut buf))?;
        Ok(outcome
            .passed
            .then(|| SenseData::parse(&buf[..outcome.transferred]))
            .flatten())
    }

    /// READ CAPACITY(10), then READ CAPACITY(16) if the device is too large;
    /// selects the read/write commands accordingly.
    fn detect_capacity(&mut self) -> Result<Capacity> {
        let capacity = self.read_capacity_10()?;
        self.use_16 = false;
        if capacity.max_lba != u64::from(u32::MAX) {
            debug!("capacity: {capacity:?}");
            return Ok(capacity);
        }

        debug!("device too large for READ CAPACITY(10), trying READ CAPACITY(16)");
        match self.read_capacity_16() {
            Ok(capacity16) => {
                debug!("capacity: {capacity16:?}, using 16-byte commands");
                self.use_16 = true;
                Ok(capacity16)
            }
            Err(err @ (Error::PhaseError | Error::InvalidCsw(_) | Error::CommandFailed { .. })) => {
                warn!("READ CAPACITY(16) failed ({err}), only the first 2^32 blocks are usable");
                Ok(capacity)
            }
            Err(err) => Err(err),
        }
    }

    /// Map a non-passing or incomplete command to an error, fetching sense
    /// data for failed ones.
    fn ensure_complete(
        &mut self,
        cdb: &Cdb,
        outcome: Outcome,
        expected: usize,
        stage: Stage,
    ) -> Result<()> {
        if !outcome.passed {
            let sense = self.request_sense().unwrap_or_else(|err| {
                warn!(
                    "REQUEST SENSE after failed command {:#04x}: {err}",
                    cdb.opcode()
                );
                None
            });
            return Err(Error::CommandFailed {
                opcode: cdb.opcode(),
                sense,
            });
        }
        if outcome.transferred != expected {
            return Err(Error::ShortTransfer {
                stage,
                expected,
                actual: outcome.transferred,
            });
        }
        Ok(())
    }

    /// GET MAX LUN. Devices with a single LUN may stall it, which means LUN 0.
    fn get_max_lun(&mut self) -> u8 {
        let mut buf = [0; 1];
        match self.handle.read_control(
            request_type(UsbDirection::In, RequestType::Class, Recipient::Interface),
            REQ_GET_MAX_LUN,
            0,
            self.interface.into(),
            &mut buf,
            self.timeout,
        ) {
            Ok(1) => buf[0].min(MAX_LUN),
            Ok(_) => 0,
            Err(err) => {
                debug!("GET MAX LUN failed ({err}), assuming a single lun");
                0
            }
        }
    }

    /// Run one Bulk-Only transaction: CBW, optional data stage, CSW.
    ///
    /// A `Failed` status is reported through [`Outcome::passed`]; transport
    /// failures, phase errors and invalid CSWs are errors, the last two after
    /// a reset recovery.
    fn execute(&mut self, cdb: &Cdb, data: Data<'_>) -> Result<Outcome> {
        let (direction, length) = match &data {
            Data::None => (Direction::Out, 0),
            Data::In(buf) => (Direction::In, buf.len()),
            Data::Out(buf) => (Direction::Out, buf.len()),
        };
        let transfer_length = u32::try_from(length).map_err(|_| Error::OutOfRange {
            what: "transfer length",
            value: length as u64,
        })?;

        self.tag = self.tag.wrapping_add(1);
        let tag = self.tag;
        let cbw = Cbw {
            tag,
            transfer_length,
            direction,
            lun: self.lun,
            cdb,
        }
        .to_bytes();
        let sent = self
            .handle
            .write_bulk(self.endpoint_out, &cbw, self.timeout)
            .map_err(Error::usb(Stage::Command))?;
        if sent != CBW_LEN {
            return Err(Error::ShortTransfer {
                stage: Stage::Command,
                expected: CBW_LEN,
                actual: sent,
            });
        }

        let transferred = match data {
            Data::None => 0,
            Data::In(buf) => self.data_in(buf)?,
            Data::Out(buf) => self.data_out(buf)?,
        };

        let csw = match self.read_csw(tag) {
            Ok(csw) => csw,
            Err(err @ Error::InvalidCsw(_)) => {
                self.reset_recovery();
                return Err(err);
            }
            Err(err) => return Err(err),
        };
        match csw.status {
            CswStatus::Passed | CswStatus::Failed => {}
            CswStatus::PhaseError => {
                self.reset_recovery();
                return Err(Error::PhaseError);
            }
        }
        if csw.residue != 0 {
            debug!("command {:#04x}: residue {}", cdb.opcode(), csw.residue);
        }
        Ok(Outcome {
            passed: csw.status == CswStatus::Passed,
            transferred,
        })
    }

    /// Receive up to `buf.len()` bytes; returns the number received.
    ///
    /// rusb reports a timeout that moved some data as a short success, so
    /// keep reading while only full packets arrive. A short packet ends the
    /// stage; so does a stall, which is cleared so the CSW can be read.
    fn data_in(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut done = 0;
        while done < buf.len() {
            match self
                .handle
                .read_bulk(self.endpoint_in, &mut buf[done..], self.timeout)
            {
                Ok(n) => {
                    done += n;
                    if n == 0 || n % self.max_packet_size != 0 {
                        break;
                    }
                }
                Err(rusb::Error::Pipe) => {
                    debug!("data-in stalled after {done} bytes");
                    self.clear_halt(self.endpoint_in);
                    break;
                }
                Err(err) => {
                    return Err(Error::Usb {
                        stage: Stage::DataIn,
                        source: err,
                    });
                }
            }
        }
        Ok(done)
    }

    /// Send `buf`; returns the number of bytes the device accepted before
    /// completing or stalling.
    fn data_out(&mut self, buf: &[u8]) -> Result<usize> {
        let mut done = 0;
        while done < buf.len() {
            match self
                .handle
                .write_bulk(self.endpoint_out, &buf[done..], self.timeout)
            {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(rusb::Error::Pipe) => {
                    debug!("data-out stalled after {done} bytes");
                    self.clear_halt(self.endpoint_out);
                    break;
                }
                Err(err) => {
                    return Err(Error::Usb {
                        stage: Stage::DataOut,
                        source: err,
                    });
                }
            }
        }
        Ok(done)
    }

    /// Read the CSW for `tag`, retrying on timeouts and clearing one stall
    fn read_csw(&mut self, tag: u32) -> Result<Csw> {
        let mut buf = [0; CSW_LEN];
        let mut stall_cleared = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self
                .handle
                .read_bulk(self.endpoint_in, &mut buf, self.timeout)
            {
                Ok(CSW_LEN) => return Csw::parse(&buf, tag),
                Ok(n) => return Err(Error::InvalidCsw(format!("{n} bytes long"))),
                Err(rusb::Error::Pipe) if !stall_cleared => {
                    debug!("status stage stalled, clearing halt");
                    stall_cleared = true;
                    self.clear_halt(self.endpoint_in);
                }
                Err(rusb::Error::Timeout) if attempt < CSW_READ_ATTEMPTS => {
                    warn!("timed out reading CSW, retrying ({attempt}/{CSW_READ_ATTEMPTS})");
                }
                Err(err) => {
                    return Err(Error::Usb {
                        stage: Stage::Status,
                        source: err,
                    });
                }
            }
        }
    }

    fn clear_halt(&self, endpoint: u8) {
        if let Err(err) = self.handle.clear_halt(endpoint) {
            warn!("clear halt on endpoint {endpoint:#04x} failed: {err}");
        }
    }

    /// Bulk-Only Mass Storage Reset followed by clearing both bulk endpoints
    /// Best effort: failures are logged.
    fn reset_recovery(&mut self) {
        warn!("performing bulk-only reset recovery");
        if let Err(err) = self.handle.write_control(
            request_type(UsbDirection::Out, RequestType::Class, Recipient::Interface),
            REQ_BULK_ONLY_RESET,
            0,
            self.interface.into(),
            &[],
            self.timeout,
        ) {
            warn!("bulk-only reset failed: {err}");
        }
        self.clear_halt(self.endpoint_in);
        self.clear_halt(self.endpoint_out);
    }
}

fn lba_10(offset: u64) -> Result<u32> {
    u32::try_from(offset).map_err(|_| Error::OutOfRange {
        what: "LBA",
        value: offset,
    })
}

fn blocks_16(count: u64) -> Result<u32> {
    u32::try_from(count).map_err(|_| Error::OutOfRange {
        what: "block count",
        value: count,
    })
}

fn blocks_10(count: u64) -> Result<u16> {
    u16::try_from(count).map_err(|_| Error::OutOfRange {
        what: "block count",
        value: count,
    })
}
