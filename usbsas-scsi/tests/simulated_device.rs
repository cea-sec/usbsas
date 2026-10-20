//! Exercises `ScsiUsb` against a simulated Bulk-Only mass storage device.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    time::Duration,
};

use usbsas_scsi::{Error, ScsiUsb, SenseKey, Support16, UsbTransport};

const EP_IN: u8 = 0x81;
const EP_OUT: u8 = 0x02;
const MPS: usize = 512;
const BLOCK: usize = 512;

#[derive(Clone)]
struct Lun {
    peripheral: u8,
    /// Reported capacity in blocks. The medium is sparse: unwritten blocks
    /// hold a pattern derived from their LBA.
    blocks: u64,
    written: HashMap<u64, Vec<u8>>,
    /// Whether READ CAPACITY(16), READ(16) and WRITE(16) are implemented.
    supports_16: bool,
    write_protected: bool,
    /// TEST UNIT READY answers "becoming ready" this many times.
    not_ready_polls: u32,
    unit_attention: bool,
    medium_present: bool,
}

fn pattern(lba: u64) -> Vec<u8> {
    let seed = (lba ^ (lba >> 32)) as u8;
    (0..BLOCK).map(|i| seed ^ i as u8).collect()
}

impl Lun {
    fn disk(blocks: u64) -> Self {
        Self {
            peripheral: 0x00,
            blocks,
            written: HashMap::new(),
            supports_16: false,
            write_protected: false,
            not_ready_polls: 0,
            unit_attention: false,
            medium_present: true,
        }
    }

    fn disk_16(blocks: u64) -> Self {
        Self {
            supports_16: true,
            ..Self::disk(blocks)
        }
    }

    fn absent() -> Self {
        Self {
            peripheral: 0x7f,
            ..Self::disk(0)
        }
    }

    fn read(&self, lba: u64, count: u64) -> Vec<u8> {
        (lba..lba + count)
            .flat_map(|lba| {
                self.written
                    .get(&lba)
                    .cloned()
                    .unwrap_or_else(|| pattern(lba))
            })
            .collect()
    }

    fn write(&mut self, lba: u64, data: &[u8]) {
        for (i, block) in data.chunks(BLOCK).enumerate() {
            self.written.insert(lba + i as u64, block.to_vec());
        }
    }
}

/// How the device answers opcodes it doesn't implement.
#[derive(Default, Clone, Copy)]
enum UnknownOpcode {
    /// CHECK CONDITION, ILLEGAL REQUEST / INVALID COMMAND OPERATION CODE.
    #[default]
    IllegalRequest,
    /// Phase error, as some broken bridges do.
    PhaseError,
}

struct PendingOut {
    tag: u32,
    lun: usize,
    lba: u64,
    expected: usize,
    received: Vec<u8>,
}

#[derive(Default)]
struct State {
    luns: Vec<Lun>,
    stall_get_max_lun: bool,
    unknown_opcode: UnknownOpcode,
    /// Bulk-in stream: data and CSWs, or injected errors.
    bulk_in: VecDeque<Result<Vec<u8>, rusb::Error>>,
    pending_out: Option<PendingOut>,
    sense: Option<(u8, u8, u8)>,
    // Fault injection.
    csw_timeouts: u32,
    phase_error_next: bool,
    /// Fail the next command (other than REQUEST SENSE) with UNIT ATTENTION.
    unit_attention_next: bool,
    short_read_next: bool,
    // Observations.
    resets: u32,
    cleared_halts: Vec<u8>,
    commands: Vec<(usize, u8)>,
}

impl State {
    fn csw(&mut self, tag: u32, residue: usize, status: u8) {
        for _ in 0..std::mem::take(&mut self.csw_timeouts) {
            self.bulk_in.push_back(Err(rusb::Error::Timeout));
        }
        let mut csw = b"USBS".to_vec();
        csw.extend(tag.to_le_bytes());
        csw.extend((residue as u32).to_le_bytes());
        csw.push(status);
        self.bulk_in.push_back(Ok(csw));
    }

    fn fail(&mut self, tag: u32, residue: usize, sense: (u8, u8, u8), stall: bool) {
        self.sense = Some(sense);
        if stall {
            self.bulk_in.push_back(Err(rusb::Error::Pipe));
        }
        self.csw(tag, residue, 1);
    }

    fn phase_error(&mut self, tag: u32, len: usize, data_in: bool) {
        if data_in && len > 0 {
            self.bulk_in.push_back(Err(rusb::Error::Pipe));
        }
        self.csw(tag, len, 2);
    }

    fn command(&mut self, cbw: &[u8]) {
        assert_eq!(cbw.len(), 31);
        assert_eq!(&cbw[0..4], b"USBC");
        let tag = u32::from_le_bytes(cbw[4..8].try_into().unwrap());
        let len = u32::from_le_bytes(cbw[8..12].try_into().unwrap()) as usize;
        let data_in = cbw[12] & 0x80 != 0;
        let lun_idx = cbw[13] as usize;
        let cdb = &cbw[15..15 + cbw[14] as usize];
        self.commands.push((lun_idx, cdb[0]));

        if std::mem::take(&mut self.phase_error_next) {
            self.phase_error(tag, len, data_in);
            return;
        }
        if cdb[0] != 0x03 && std::mem::take(&mut self.unit_attention_next) {
            self.fail(tag, len, (0x6, 0x29, 0x00), data_in);
            return;
        }
        let lun = &mut self.luns[lun_idx];
        if matches!(cdb[0], 0x9e | 0x88 | 0x8a) && !lun.supports_16 {
            match self.unknown_opcode {
                UnknownOpcode::IllegalRequest => self.fail(tag, len, (0x5, 0x20, 0x00), data_in),
                UnknownOpcode::PhaseError => self.phase_error(tag, len, data_in),
            }
            return;
        }
        match cdb[0] {
            0x12 => {
                let mut data = vec![0; 36];
                data[0] = lun.peripheral;
                data.truncate(len.min(cdb[4] as usize));
                let sent = data.len();
                self.bulk_in.push_back(Ok(data));
                self.csw(tag, len - sent, 0);
            }
            0x00 => {
                let failure = if std::mem::take(&mut lun.unit_attention) {
                    Some((0x6, 0x28, 0x00))
                } else if lun.not_ready_polls > 0 {
                    lun.not_ready_polls -= 1;
                    Some((0x2, 0x04, 0x01))
                } else if !lun.medium_present {
                    Some((0x2, 0x3a, 0x00))
                } else {
                    None
                };
                match failure {
                    Some(sense) => self.fail(tag, 0, sense, false),
                    None => self.csw(tag, 0, 0),
                }
            }
            0x03 => {
                let (key, asc, ascq) = self.sense.take().unwrap_or_default();
                let mut data = vec![0; 18];
                data[0] = 0x70;
                data[2] = key;
                data[7] = 10;
                data[12] = asc;
                data[13] = ascq;
                data.truncate(len);
                self.bulk_in.push_back(Ok(data));
                self.csw(tag, 0, 0);
            }
            0x25 => {
                let max_lba = (lun.blocks - 1).min(u64::from(u32::MAX)) as u32;
                let mut data = max_lba.to_be_bytes().to_vec();
                data.extend((BLOCK as u32).to_be_bytes());
                self.bulk_in.push_back(Ok(data));
                self.csw(tag, 0, 0);
            }
            0x9e => {
                assert_eq!(cdb[1], 0x10, "only READ CAPACITY(16) is simulated");
                let alloc = u32::from_be_bytes(cdb[10..14].try_into().unwrap()) as usize;
                let mut data = (lun.blocks - 1).to_be_bytes().to_vec();
                data.extend((BLOCK as u32).to_be_bytes());
                data.resize(32, 0);
                data.truncate(alloc.min(len));
                let sent = data.len();
                self.bulk_in.push_back(Ok(data));
                self.csw(tag, len - sent, 0);
            }
            0x28 | 0x2a | 0x88 | 0x8a => {
                let (lba, blocks) = if cdb.len() == 16 {
                    (
                        u64::from_be_bytes(cdb[2..10].try_into().unwrap()),
                        u64::from(u32::from_be_bytes(cdb[10..14].try_into().unwrap())),
                    )
                } else {
                    (
                        u64::from(u32::from_be_bytes(cdb[2..6].try_into().unwrap())),
                        u64::from(u16::from_be_bytes(cdb[7..9].try_into().unwrap())),
                    )
                };
                let write = matches!(cdb[0], 0x2a | 0x8a);
                if lba + blocks > lun.blocks || blocks as usize * BLOCK != len {
                    // LBA out of range: stall the data stage.
                    self.fail(tag, len, (0x5, 0x21, 0x00), data_in);
                    return;
                }
                if write && lun.write_protected {
                    assert_eq!(len, 0, "write-protect simulated for empty writes only");
                    self.fail(tag, 0, (0x7, 0x27, 0x00), false);
                    return;
                }
                if blocks == 0 {
                    self.csw(tag, 0, 0);
                    return;
                }
                if write {
                    self.pending_out = Some(PendingOut {
                        tag,
                        lun: lun_idx,
                        lba,
                        expected: len,
                        received: Vec::new(),
                    });
                    return;
                }
                let mut data = lun.read(lba, blocks);
                if std::mem::take(&mut self.short_read_next) {
                    // Device ends the data stage early with a short packet.
                    data.truncate(BLOCK + 100);
                }
                let residue = len - data.len();
                self.bulk_in.push_back(Ok(data));
                self.csw(tag, residue, 0);
            }
            other => panic!("unexpected opcode {other:#04x}"),
        }
    }
}

#[derive(Default)]
struct SimulatedDevice(RefCell<State>);

impl SimulatedDevice {
    fn new(luns: Vec<Lun>) -> Self {
        Self(RefCell::new(State {
            luns,
            ..Default::default()
        }))
    }
}

impl UsbTransport for SimulatedDevice {
    fn read_bulk(&self, endpoint: u8, buf: &mut [u8], _: Duration) -> rusb::Result<usize> {
        assert_eq!(endpoint, EP_IN);
        let mut state = self.0.borrow_mut();
        match state
            .bulk_in
            .pop_front()
            .expect("host read with nothing queued")
        {
            Err(err) => Err(err),
            Ok(mut data) => {
                let n = data.len().min(buf.len());
                buf[..n].copy_from_slice(&data[..n]);
                if n < data.len() {
                    state.bulk_in.push_front(Ok(data.split_off(n)));
                }
                Ok(n)
            }
        }
    }

    fn write_bulk(&self, endpoint: u8, buf: &[u8], _: Duration) -> rusb::Result<usize> {
        assert_eq!(endpoint, EP_OUT);
        let mut state = self.0.borrow_mut();
        let Some(mut pending) = state.pending_out.take() else {
            state.command(buf);
            return Ok(buf.len());
        };
        pending.received.extend_from_slice(buf);
        assert!(pending.received.len() <= pending.expected);
        if pending.received.len() == pending.expected {
            state.luns[pending.lun].write(pending.lba, &pending.received);
            state.csw(pending.tag, 0, 0);
        } else {
            state.pending_out = Some(pending);
        }
        Ok(buf.len())
    }

    fn read_control(
        &self,
        _: u8,
        request: u8,
        _: u16,
        _: u16,
        buf: &mut [u8],
        _: Duration,
    ) -> rusb::Result<usize> {
        assert_eq!(request, 0xfe);
        let state = self.0.borrow();
        if state.stall_get_max_lun {
            return Err(rusb::Error::Pipe);
        }
        buf[0] = state.luns.len() as u8 - 1;
        Ok(1)
    }

    fn write_control(
        &self,
        _: u8,
        request: u8,
        _: u16,
        _: u16,
        _: &[u8],
        _: Duration,
    ) -> rusb::Result<usize> {
        assert_eq!(request, 0xff);
        let mut state = self.0.borrow_mut();
        state.resets += 1;
        state.bulk_in.clear();
        state.pending_out = None;
        Ok(0)
    }

    fn clear_halt(&self, endpoint: u8) -> rusb::Result<()> {
        self.0.borrow_mut().cleared_halts.push(endpoint);
        Ok(())
    }

    fn set_alternate_setting(&self, _: u8, _: u8) -> rusb::Result<()> {
        Err(rusb::Error::NotSupported)
    }

    fn reset(&self) -> rusb::Result<()> {
        Ok(())
    }

    fn max_packet_size(&self, endpoint: u8) -> Option<u16> {
        (endpoint == EP_IN).then_some(MPS as u16)
    }
}

fn scsi(luns: Vec<Lun>) -> ScsiUsb<SimulatedDevice> {
    ScsiUsb::new(
        SimulatedDevice::new(luns),
        0,
        0,
        EP_IN,
        EP_OUT,
        Duration::from_secs(1),
    )
}

fn state<'a>(scsi: &'a ScsiUsb<SimulatedDevice>) -> std::cell::RefMut<'a, State> {
    scsi.handle().0.borrow_mut()
}

#[test]
fn init_waits_for_unit_attention_and_becoming_ready() {
    let mut lun = Lun::disk(100);
    lun.unit_attention = true;
    lun.not_ready_polls = 2;
    let mut scsi = scsi(vec![lun]);
    let capacity = scsi.init_mass_storage().unwrap();
    assert_eq!(capacity.max_lba, 99);
    assert_eq!(capacity.block_size, 512);
    assert_eq!(capacity.device_size(), 100 * 512);
    assert!(state(&scsi).bulk_in.is_empty());
}

#[test]
fn init_skips_absent_and_not_present_luns() {
    let mut no_medium = Lun::disk(10);
    no_medium.medium_present = false;
    let mut scsi = scsi(vec![Lun::absent(), no_medium, Lun::disk(42)]);
    let capacity = scsi.init_mass_storage().unwrap();
    assert_eq!(scsi.lun(), 2);
    assert_eq!(capacity.max_lba, 41);
    // The absent LUN never received TEST UNIT READY.
    assert!(!state(&scsi).commands.contains(&(0, 0x00)));
}

#[test]
fn init_without_ready_lun_fails() {
    let mut lun = Lun::disk(10);
    lun.medium_present = false;
    let mut scsi = scsi(vec![lun]);
    state(&scsi).stall_get_max_lun = true;
    assert!(matches!(scsi.init_mass_storage(), Err(Error::NoReadyLun)));
}

#[test]
fn read_sectors_spans_multiple_commands() {
    let blocks = 2 * usize::from(usbsas_scsi::MAX_SECTORS_PER_TRANSFER) + 10;
    let mut scsi = scsi(vec![Lun::disk(blocks as u64 + 5)]);
    scsi.init_mass_storage().unwrap();
    let data = scsi.read_sectors(3, blocks as u64, BLOCK).unwrap();
    assert_eq!(data, state(&scsi).luns[0].read(3, blocks as u64));
    let reads = state(&scsi).commands.iter().filter(|c| c.1 == 0x28).count();
    assert_eq!(reads, 3);
}

#[test]
fn write_then_read_back() {
    let mut scsi = scsi(vec![Lun::disk(64)]);
    scsi.init_mass_storage().unwrap();
    let payload: Vec<u8> = (0..4 * BLOCK).map(|i| (i % 251) as u8).collect();
    scsi.scsi_write_10(&payload, 10, 4).unwrap();
    assert_eq!(scsi.read_sectors(10, 4, BLOCK).unwrap(), payload);
}

#[test]
fn out_of_range_read_reports_sense_and_recovers() {
    let mut scsi = scsi(vec![Lun::disk(8)]);
    scsi.init_mass_storage().unwrap();
    let mut buf = vec![0; 2 * BLOCK];
    let err = scsi.scsi_read_10(&mut buf, 7, 2).unwrap_err();
    match err {
        Error::CommandFailed {
            opcode: 0x28,
            sense: Some(sense),
        } => {
            assert_eq!(sense.key, SenseKey::IllegalRequest);
            assert_eq!(sense.asc, 0x21);
        }
        other => panic!("unexpected error {other:?}"),
    }
    assert_eq!(state(&scsi).cleared_halts, [EP_IN]);
    // The pipe is still in sync.
    scsi.scsi_read_10(&mut buf, 6, 2).unwrap();
}

#[test]
fn short_read_is_an_error_and_keeps_pipe_in_sync() {
    let mut scsi = scsi(vec![Lun::disk(8)]);
    scsi.init_mass_storage().unwrap();
    state(&scsi).short_read_next = true;
    let mut buf = vec![0; 4 * BLOCK];
    assert!(matches!(
        scsi.scsi_read_10(&mut buf, 0, 4),
        Err(Error::ShortTransfer { actual, .. }) if actual == BLOCK + 100
    ));
    scsi.scsi_read_10(&mut buf, 0, 4).unwrap();
}

#[test]
fn csw_timeouts_are_retried() {
    let mut scsi = scsi(vec![Lun::disk(8)]);
    scsi.init_mass_storage().unwrap();
    let mut buf = vec![0; BLOCK];
    state(&scsi).csw_timeouts = 2;
    scsi.scsi_read_10(&mut buf, 0, 1).unwrap();
    state(&scsi).csw_timeouts = 3;
    assert!(matches!(
        scsi.scsi_read_10(&mut buf, 0, 1),
        Err(Error::Usb {
            source: rusb::Error::Timeout,
            ..
        })
    ));
}

#[test]
fn phase_error_triggers_reset_recovery() {
    let mut scsi = scsi(vec![Lun::disk(8)]);
    scsi.init_mass_storage().unwrap();
    state(&scsi).phase_error_next = true;
    let mut buf = vec![0; BLOCK];
    assert!(matches!(
        scsi.scsi_read_10(&mut buf, 0, 1),
        Err(Error::PhaseError)
    ));
    {
        let state = state(&scsi);
        assert_eq!(state.resets, 1);
        // Data-stage stall, then both endpoints during reset recovery.
        assert_eq!(state.cleared_halts, [EP_IN, EP_IN, EP_OUT]);
    }
    scsi.scsi_read_10(&mut buf, 0, 1).unwrap();
}

#[test]
fn argument_validation() {
    let mut scsi = scsi(vec![Lun::disk(8)]);
    let mut buf = vec![0; BLOCK];
    assert!(matches!(
        scsi.scsi_read_10(&mut buf, 1 << 32, 1),
        Err(Error::OutOfRange { what: "LBA", .. })
    ));
    assert!(matches!(
        scsi.scsi_write_10(&buf, 0, 1 << 16),
        Err(Error::OutOfRange {
            what: "block count",
            ..
        })
    ));
    assert!(matches!(
        scsi.read_sectors(0, 1, 0),
        Err(Error::InvalidBlockSize(0))
    ));
    assert!(scsi.read_sectors(0, 0, BLOCK).unwrap().is_empty());
    // Nothing reached the device.
    assert!(state(&scsi).commands.is_empty());
}

const TEN_BYTE_LIMIT: u64 = 1 << 32;

fn opcodes(scsi: &ScsiUsb<SimulatedDevice>) -> Vec<u8> {
    state(scsi).commands.iter().map(|c| c.1).collect()
}

#[test]
fn small_device_never_sees_16_byte_commands() {
    let mut scsi = scsi(vec![Lun::disk_16(1000)]);
    scsi.init_mass_storage().unwrap();
    assert!(!scsi.uses_16_byte_commands());
    let payload = vec![0x5a; BLOCK];
    scsi.write_blocks(&payload, 7, 1).unwrap();
    assert_eq!(scsi.read_sectors(7, 1, BLOCK).unwrap(), payload);
    assert!(
        opcodes(&scsi)
            .iter()
            .all(|op| !matches!(op, 0x9e | 0x88 | 0x8a))
    );
}

#[test]
fn large_device_switches_to_16_byte_commands() {
    let blocks = TEN_BYTE_LIMIT + 100;
    let mut scsi = scsi(vec![Lun::disk_16(blocks)]);
    let capacity = scsi.init_mass_storage().unwrap();
    assert_eq!(capacity.max_lba, blocks - 1);
    assert_eq!(capacity.device_size(), blocks * BLOCK as u64);
    assert!(scsi.uses_16_byte_commands());

    // Blocks beyond the 32-bit boundary, crossing it too.
    let lba = TEN_BYTE_LIMIT - 2;
    let payload: Vec<u8> = (0..4 * BLOCK).map(|i| (i % 253) as u8).collect();
    scsi.write_blocks(&payload, lba, 4).unwrap();
    assert_eq!(scsi.read_sectors(lba, 4, BLOCK).unwrap(), payload);
    let last = scsi.read_sectors(blocks - 1, 1, BLOCK).unwrap();
    assert_eq!(last, pattern(blocks - 1));

    let ops = opcodes(&scsi);
    assert!(ops.contains(&0x9e) && ops.contains(&0x88) && ops.contains(&0x8a));
    assert!(!ops.contains(&0x28) && !ops.contains(&0x2a));
}

#[test]
fn large_device_without_16_byte_support_falls_back() {
    for unknown in [UnknownOpcode::IllegalRequest, UnknownOpcode::PhaseError] {
        let mut scsi = scsi(vec![Lun::disk(TEN_BYTE_LIMIT + 100)]);
        state(&scsi).unknown_opcode = unknown;
        let capacity = scsi.init_mass_storage().unwrap();
        assert_eq!(capacity.max_lba, u64::from(u32::MAX));
        assert!(!scsi.uses_16_byte_commands());
        // Still usable within the 32-bit range, and the pipe is in sync.
        let data = scsi.read_sectors(TEN_BYTE_LIMIT - 1, 1, BLOCK).unwrap();
        assert_eq!(data, pattern(TEN_BYTE_LIMIT - 1));
        let mut buf = vec![0; BLOCK];
        assert!(matches!(
            scsi.read_blocks(&mut buf, TEN_BYTE_LIMIT, 1),
            Err(Error::OutOfRange { what: "LBA", .. })
        ));
    }
}

#[test]
fn detect_on_supporting_device() {
    let mut scsi = scsi(vec![Lun::disk_16(1000)]);
    scsi.init_mass_storage().unwrap();
    let written_before = state(&scsi).luns[0].written.len();
    let support = scsi.detect_16_byte_support().unwrap();
    assert!(support.all());
    // Detection doesn't switch, and doesn't touch the medium.
    assert!(!scsi.uses_16_byte_commands());
    assert_eq!(state(&scsi).luns[0].written.len(), written_before);

    scsi.set_use_16_byte_commands(true);
    let payload = vec![0xa5; 2 * BLOCK];
    scsi.write_blocks(&payload, 10, 2).unwrap();
    assert_eq!(scsi.read_sectors(10, 2, BLOCK).unwrap(), payload);
}

#[test]
fn detect_on_unsupporting_device() {
    for unknown in [UnknownOpcode::IllegalRequest, UnknownOpcode::PhaseError] {
        let mut scsi = scsi(vec![Lun::disk(1000)]);
        state(&scsi).unknown_opcode = unknown;
        scsi.init_mass_storage().unwrap();
        assert_eq!(scsi.detect_16_byte_support().unwrap(), Support16::default());
        let resets = state(&scsi).resets;
        match unknown {
            UnknownOpcode::IllegalRequest => assert_eq!(resets, 0),
            UnknownOpcode::PhaseError => assert_eq!(resets, 3),
        }
        // The device is still usable with 10-byte commands.
        assert_eq!(scsi.read_sectors(1, 1, BLOCK).unwrap(), pattern(1));
    }
}

#[test]
fn detect_on_write_protected_device() {
    let mut lun = Lun::disk_16(1000);
    lun.write_protected = true;
    let mut scsi = scsi(vec![lun]);
    scsi.init_mass_storage().unwrap();
    // DATA PROTECT means WRITE(16) was understood.
    assert!(scsi.detect_16_byte_support().unwrap().all());
}

#[test]
fn detect_retries_after_unit_attention() {
    let mut scsi = scsi(vec![Lun::disk_16(1000)]);
    scsi.init_mass_storage().unwrap();
    state(&scsi).unit_attention_next = true;
    assert!(scsi.detect_16_byte_support().unwrap().all());
    let rc16 = opcodes(&scsi).iter().filter(|&&op| op == 0x9e).count();
    assert_eq!(rc16, 2);
}

#[test]
fn read_16_argument_validation() {
    let mut scsi = scsi(vec![Lun::disk_16(8)]);
    let buf = vec![0; BLOCK];
    assert!(matches!(
        scsi.scsi_write_16(&buf, 0, 1 << 32),
        Err(Error::OutOfRange {
            what: "block count",
            ..
        })
    ));
    assert!(state(&scsi).commands.is_empty());
}
