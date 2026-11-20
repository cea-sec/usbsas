//! Unit tests: the process runs in a thread on a `MockMassStorage` and is driven
//! over pipes as scsi2files does. The device is `mock_in_dev.img` of the
//! integration tests, or a disk image built in memory for the layouts it
//! doesn't have.

use super::*;
use flate2::bufread::GzDecoder;
use std::{
    fs::{self, File},
    io::{self, BufReader, Read},
    os::fd::OwnedFd,
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use usbsas_comm::{ComRqScsi, Comm, ProtoReqCommon, ProtoReqScsi, SendRecv};
use usbsas_proto::scsi::{
    Request, RequestEnd, RequestPartitions, RequestReadSectors, Response, response,
};

const SECTOR: usize = 512;
const MIB: usize = 1 << 20;

// Disk images

/// Disk image built in memory.
struct Img(Vec<u8>);

impl Img {
    fn new(size: usize) -> Self {
        Img(vec![0; size])
    }

    fn put(&mut self, offset: usize, bytes: &[u8]) -> &mut Self {
        self.0[offset..offset + bytes.len()].copy_from_slice(bytes);
        self
    }

    fn put_u32(&mut self, offset: usize, val: u32) -> &mut Self {
        self.put(offset, &val.to_le_bytes())
    }

    /// FAT12/16 boot sector at `offset`. The volume serial is in front of the
    /// label and isn't valid UTF-8 on purpose.
    fn fat16(&mut self, offset: usize, label: &[u8; 11]) -> &mut Self {
        self.put(offset, &[0xEB, 0x3C, 0x90])
            .put(offset + 0x3, b"MSWIN4.1")
            .put(offset + 0x26, &[0x29, 0xE1, 0x9C, 0x3F, 0xA2])
            .put(offset + 0x2B, label)
            .put(offset + 0x36, b"FAT16   ")
            .put(offset + 0x1FE, &[0x55, 0xAA])
    }

    /// FAT32 boot sector at `offset`.
    fn fat32(&mut self, offset: usize, label: &[u8; 11]) -> &mut Self {
        self.put(offset, &[0xEB, 0x58, 0x90])
            .put(offset + 0x3, b"mkfs.fat")
            .put(offset + 0x42, &[0x29, 0xE1, 0x9C, 0x3F, 0xA2])
            .put(offset + 0x47, label)
            .put(offset + 0x52, b"FAT32   ")
            .put(offset + 0x1FE, &[0x55, 0xAA])
    }

    fn exfat(&mut self, offset: usize) -> &mut Self {
        self.put(offset, &[0xEB, 0x76, 0x90])
            .put(offset + 0x3, b"EXFAT   ")
            .put(offset + 0x1FE, &[0x55, 0xAA])
    }

    fn ntfs(&mut self, offset: usize) -> &mut Self {
        self.put(offset, &[0xEB, 0x52, 0x90])
            .put(offset + 0x3, b"NTFS    ")
            .put(offset + 0x1FE, &[0x55, 0xAA])
    }

    /// ext2/3/4 superblock (at `offset` + 1024) with `incompat` features.
    fn ext(&mut self, offset: usize, incompat: u32, label: &[u8]) -> &mut Self {
        self.put(offset + 0x438, &[0x53, 0xEF])
            .put_u32(offset + 0x460, incompat)
            .put(offset + 0x478, label)
    }

    /// ISO9660 primary volume descriptor (at `offset` + 32K).
    fn iso(&mut self, offset: usize) -> &mut Self {
        self.put(offset + 0x8000, b"\x01CD001\x01")
            .put(offset + 0x8800, b"\xffCD001\x01")
    }

    /// MBR with `(type, first sector, sector count)` entries.
    fn mbr(&mut self, parts: &[(u8, u32, u32)]) -> &mut Self {
        for (i, (ptype, start, len)) in parts.iter().enumerate() {
            let entry = 446 + 16 * i;
            self.put(entry + 4, &[*ptype])
                .put_u32(entry + 8, *start)
                .put_u32(entry + 12, *len);
        }
        self.put(510, &[0x55, 0xAA])
    }

    /// Protective MBR and primary GPT with `(first lba, last lba, name)`
    /// entries.
    fn gpt(&mut self, sector: usize, parts: &[(u64, u64, &str)]) -> &mut Self {
        let last_lba = (self.0.len() / sector - 1) as u64;
        self.mbr(&[(0xEE, 1, u32::try_from(last_lba).unwrap())]);

        // 128 entries of 128 bytes
        let mut table = vec![0u8; 128 * 128];
        for (i, (first, last, name)) in parts.iter().enumerate() {
            let entry = &mut table[i * 128..(i + 1) * 128];
            // Microsoft basic data
            entry[0..16].copy_from_slice(&[
                0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26,
                0x99, 0xC7,
            ]);
            entry[16] = i as u8 + 1;
            entry[0x20..0x28].copy_from_slice(&first.to_le_bytes());
            entry[0x28..0x30].copy_from_slice(&last.to_le_bytes());
            for (j, c) in name.encode_utf16().enumerate() {
                entry[0x38 + 2 * j..0x38 + 2 * j + 2].copy_from_slice(&c.to_le_bytes());
            }
        }
        let first_usable = 2 + (table.len() / sector) as u64;

        let mut header = vec![0u8; 92];
        header[0..8].copy_from_slice(b"EFI PART");
        header[8..12].copy_from_slice(&[0, 0, 1, 0]);
        header[12..16].copy_from_slice(&92u32.to_le_bytes());
        header[0x18..0x20].copy_from_slice(&1u64.to_le_bytes());
        header[0x20..0x28].copy_from_slice(&last_lba.to_le_bytes());
        header[0x28..0x30].copy_from_slice(&first_usable.to_le_bytes());
        header[0x30..0x38].copy_from_slice(&(last_lba - first_usable).to_le_bytes());
        header[0x38] = 0x42;
        header[0x48..0x50].copy_from_slice(&2u64.to_le_bytes());
        header[0x50..0x54].copy_from_slice(&128u32.to_le_bytes());
        header[0x54..0x58].copy_from_slice(&128u32.to_le_bytes());
        header[0x58..0x5C].copy_from_slice(&crc32(&table).to_le_bytes());
        let crc = crc32(&header);
        header[0x10..0x14].copy_from_slice(&crc.to_le_bytes());

        self.put(sector, &header).put(2 * sector, &table)
    }

    fn build(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

/// CRC-32 (ISO-HDLC), used by GPT.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// `mock_in_dev.img` of the integration tests (MBR with NTFS, FAT12 and ext4
/// partitions), decompressed once.
fn mock_in_dev() -> &'static [u8] {
    static IMG: OnceLock<Vec<u8>> = OnceLock::new();
    IMG.get_or_init(|| {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../usbsas-usbsas/tests/resources/mock_in_dev.img.gz"
        );
        let mut img = Vec::new();
        GzDecoder::new(BufReader::new(File::open(path).unwrap()))
            .read_to_end(&mut img)
            .unwrap();
        img
    })
}

/// Write `img` to a temporary file, which is removed when dropped.
struct TmpDev(PathBuf);

impl TmpDev {
    fn new(img: &[u8]) -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "usbsas-dev2scsi-test-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::write(&path, img).unwrap();
        TmpDev(path)
    }

    fn open(&self) -> File {
        File::open(&self.0).unwrap()
    }
}

impl Drop for TmpDev {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

// Running the process

fn pipe_files() -> (File, File) {
    let (r, w) = io::pipe().unwrap();
    (File::from(OwnedFd::from(r)), File::from(OwnedFd::from(w)))
}

/// The process running in a thread, and its parent's end of the channel.
struct Running {
    rq: ComRqScsi,
    handle: JoinHandle<Result<()>>,
}

impl Running {
    fn spawn(state: State) -> Self {
        let (rq_in, rp_out) = pipe_files();
        let (rp_in, rq_out) = pipe_files();
        let dev2scsi = Dev2Scsi {
            comm: Comm::new(rp_in, rp_out),
            state,
        };
        Running {
            rq: Comm::new(rq_in, rq_out),
            handle: thread::spawn(move || dev2scsi.main_loop()),
        }
    }

    /// Process with `img` already opened, as a device with `block_size`
    /// sectors.
    fn opened_bs(img: &[u8], block_size: u32) -> Self {
        let dev = TmpDev::new(img);
        let mut usb_mass_storage = MassStorage::from_opened_file(dev.open()).unwrap();
        usb_mass_storage.block_size = block_size;
        Self::spawn(State::DevOpened(DevOpenedState { usb_mass_storage }))
    }

    fn opened(img: &[u8]) -> Self {
        Self::opened_bs(img, SECTOR as u32)
    }

    /// Process waiting for its unlock value.
    fn locked() -> Self {
        Self::spawn(State::Init(InitState {}))
    }

    fn unlock(&mut self, busnum: u32, devnum: u32) {
        let val = (u64::from(devnum) << 32) | u64::from(busnum);
        io::Write::write_all(&mut self.rq, &val.to_le_bytes()).unwrap();
    }

    fn recv(&mut self) -> response::Msg {
        let rep: Response = self.rq.recv().unwrap();
        rep.msg.unwrap()
    }

    fn partitions(&mut self) -> io::Result<Vec<PartitionInfo>> {
        Ok(self.rq.partitions(RequestPartitions {})?.partitions)
    }

    fn read(&mut self, offset: u64, count: u64) -> io::Result<Vec<u8>> {
        Ok(self
            .rq
            .readsectors(RequestReadSectors { offset, count })?
            .data)
    }

    /// Send `End`, wait for the response and for the process to exit.
    fn end(mut self) {
        self.rq.end().unwrap();
        self.join().unwrap();
    }

    fn join(self) -> Result<()> {
        drop(self.rq);
        self.handle.join().unwrap()
    }
}

/// Run `f` in a thread and fail if it doesn't return in time (instead of
/// blocking the test suite forever).
fn timeout<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("no response from dev2scsi")
}

/// List the partitions of `img` then end the process.
fn list_bs(img: &[u8], block_size: u32) -> io::Result<Vec<PartitionInfo>> {
    let mut dev2scsi = Running::opened_bs(img, block_size);
    let parts = dev2scsi.partitions();
    dev2scsi.end();
    parts
}

fn list(img: &[u8]) -> io::Result<Vec<PartitionInfo>> {
    list_bs(img, SECTOR as u32)
}

fn part(ptype: u32, start: u64, size: u64, name: &str, fs: &str) -> PartitionInfo {
    PartitionInfo {
        ptype,
        start,
        size,
        name_str: name.into(),
        type_str: fs.into(),
    }
}

// Init state

#[test]
fn unlock_zero_waits_end() {
    let mut dev2scsi = Running::locked();
    dev2scsi.unlock(0, 0);
    dev2scsi.end();
}

#[test]
fn wait_end_answers_requests_until_end() {
    let mut dev2scsi = Running::locked();
    dev2scsi.unlock(0, 0);
    let err = dev2scsi.read(0, 1).unwrap_err();
    assert_eq!(err.to_string(), "unexpected request");
    let err = dev2scsi.partitions().unwrap_err();
    assert_eq!(err.to_string(), "unexpected request");
    dev2scsi.end();
}

#[test]
fn parent_gone_before_unlock() {
    let dev2scsi = Running::locked();
    // read_exact fails, the error can't be sent either
    assert!(dev2scsi.join().is_err());
}

/// The only test using `USBSAS_MOCK_IN_DEV`, as the environment is shared by
/// all tests.
#[test]
fn unlock_opens_device() {
    let img = mock_in_dev();
    let dev = TmpDev::new(img);

    // SAFETY: no other test reads or writes the environment
    unsafe { std::env::remove_var("USBSAS_MOCK_IN_DEV") };
    let mut dev2scsi = Running::locked();
    dev2scsi.unlock(1, 2);
    match dev2scsi.recv() {
        response::Msg::Error(err) => assert!(err.err.starts_with("var error"), "{}", err.err),
        rep => panic!("unexpected response {rep:?}"),
    }
    dev2scsi.end();

    // SAFETY: see above
    unsafe { std::env::set_var("USBSAS_MOCK_IN_DEV", &dev.0) };
    let mut dev2scsi = Running::locked();
    dev2scsi.unlock(1, 2);
    match dev2scsi.recv() {
        response::Msg::OpenDevice(rep) => {
            assert_eq!(rep.block_size, 512);
            assert_eq!(rep.dev_size, img.len() as u64);
        }
        rep => panic!("unexpected response {rep:?}"),
    }
    assert_eq!(dev2scsi.read(3, 2).unwrap(), img[3 * SECTOR..5 * SECTOR]);
    dev2scsi.end();
}

// Requests

#[test]
fn read_sectors() {
    let img = mock_in_dev();
    let mut dev2scsi = Running::opened(img);
    assert_eq!(dev2scsi.read(0, 1).unwrap(), img[..SECTOR]);
    // FAT partition boot sector
    assert_eq!(
        dev2scsi.read(18432, 3).unwrap(),
        img[18432 * SECTOR..18435 * SECTOR]
    );
    let last = (img.len() / SECTOR) as u64 - 1;
    assert_eq!(dev2scsi.read(last, 1).unwrap(), img[img.len() - SECTOR..]);
    assert!(dev2scsi.read(0, 0).unwrap().is_empty());
    dev2scsi.end();
}

#[test]
fn read_sectors_out_of_device() {
    let img = mock_in_dev();
    let sectors = (img.len() / SECTOR) as u64;
    let mut dev2scsi = Running::opened(img);
    assert!(dev2scsi.read(sectors, 1).is_err());
    assert!(dev2scsi.read(sectors - 1, 2).is_err());
    assert!(dev2scsi.read(u64::MAX, 2).is_err());
    assert!(dev2scsi.read(1, u64::MAX).is_err());
    // the process is still usable
    assert_eq!(dev2scsi.read(1, 1).unwrap(), img[SECTOR..2 * SECTOR]);
    dev2scsi.end();
}

#[test]
fn read_sectors_larger_than_a_message() {
    // the response wouldn't be accepted by the parent: refuse it before
    // reading (and allocating) anything
    let img = mock_in_dev();
    let mut dev2scsi = Running::opened(img);
    let count = usbsas_utils::MAX_PROTO_MESS_SIZE / SECTOR as u64;
    let err = dev2scsi.read(0, count).unwrap_err();
    assert!(err.to_string().contains("too many sectors"), "{err}");
    assert_eq!(dev2scsi.read(1, 1).unwrap(), img[SECTOR..2 * SECTOR]);
    dev2scsi.end();
}

#[test]
fn read_sectors_after_partitions() {
    let img = mock_in_dev();
    let mut dev2scsi = Running::opened(img);
    dev2scsi.partitions().unwrap();
    assert_eq!(dev2scsi.read(0, 2).unwrap(), img[..2 * SECTOR]);
    assert!(dev2scsi.read(u64::MAX, 1).is_err());
    assert_eq!(dev2scsi.read(1, 1).unwrap(), img[SECTOR..2 * SECTOR]);
    dev2scsi.end();
}

#[test]
fn partitions_only_once() {
    let img = mock_in_dev();
    let mut dev2scsi = Running::opened(img);
    dev2scsi.partitions().unwrap();
    let mut dev2scsi = timeout(move || {
        let err = dev2scsi.partitions().unwrap_err();
        assert_eq!(err.to_string(), "unexpected request");
        dev2scsi
    });
    assert_eq!(dev2scsi.read(0, 1).unwrap(), img[..SECTOR]);
    dev2scsi.end();
}

#[test]
fn partitions_error_can_be_retried() {
    let img = Img::new(MIB).build();
    let mut dev2scsi = Running::opened(&img);
    let err = dev2scsi.partitions().unwrap_err();
    assert_eq!(
        err.to_string(),
        "partition error: No supported filesystem found"
    );
    // still in the same state
    assert_eq!(dev2scsi.read(0, 1).unwrap(), img[..SECTOR]);
    assert!(dev2scsi.partitions().is_err());
    dev2scsi.end();
}

#[test]
fn malformed_request_then_end() {
    let img = mock_in_dev();
    let mut dev2scsi = Running::opened(img);
    dev2scsi.rq.send(Request { msg: None }).unwrap();
    assert!(matches!(dev2scsi.recv(), response::Msg::Error(_)));
    // device closed, waiting for end
    let err = dev2scsi.read(0, 1).unwrap_err();
    assert_eq!(err.to_string(), "unexpected request");
    dev2scsi.end();
}

#[test]
fn end_without_partitions() {
    let dev2scsi = Running::opened(mock_in_dev());
    dev2scsi.end();
}

#[test]
fn parent_gone() {
    let dev2scsi = Running::opened(mock_in_dev());
    assert!(dev2scsi.join().is_err());
}

#[test]
fn end_request_is_answered() {
    let mut dev2scsi = Running::opened(mock_in_dev());
    dev2scsi
        .rq
        .send(Request {
            msg: Some(Msg::End(RequestEnd {})),
        })
        .unwrap();
    assert!(matches!(dev2scsi.recv(), response::Msg::End(_)));
    dev2scsi.join().unwrap();
}

// Partitions of mock_in_dev.img

#[test]
fn mock_in_dev_partitions() {
    let size = 8 * MIB as u64;
    assert_eq!(
        list(mock_in_dev()).unwrap(),
        [
            part(0x7, 2048, size, "Unknown", "NTFS"),
            // FAT12
            part(0xb, 18432, size, "TSET", "FAT"),
            part(0x83, 34816, size, "tset", "Linux/Ext"),
        ]
    );
}

// File systems on the whole device (no partition table)

#[test]
fn whole_device_fat16() {
    let img = Img::new(MIB).fat16(0, b"MY KEY     ").build();
    assert_eq!(
        list(&img).unwrap(),
        [part(0x6, 0, MIB as u64, "MY KEY", "FAT")]
    );
}

#[test]
fn whole_device_fat32() {
    let img = Img::new(MIB).fat32(0, b"MY KEY     ").build();
    assert_eq!(
        list(&img).unwrap(),
        [part(0xb, 0, MIB as u64, "MY KEY", "FAT")]
    );
}

#[test]
fn whole_device_exfat() {
    let img = Img::new(MIB).exfat(0).build();
    assert_eq!(
        list(&img).unwrap(),
        [part(0x7, 0, MIB as u64, "Unknown", "EXFAT")]
    );
}

#[test]
fn whole_device_ntfs() {
    let img = Img::new(MIB).ntfs(0).build();
    assert_eq!(
        list(&img).unwrap(),
        [part(0x7, 0, MIB as u64, "Unknown", "NTFS")]
    );
}

#[test]
fn whole_device_ext4() {
    let img = Img::new(MIB).ext(0, 0x2C2, b"rootfs").build();
    assert_eq!(
        list(&img).unwrap(),
        [part(0x83, 0, MIB as u64, "rootfs", "Linux/Ext")]
    );
}

#[test]
fn whole_device_ext3_unsupported() {
    let img = Img::new(MIB).ext(0, 0x6, b"old").build();
    let err = list(&img).unwrap_err();
    assert_eq!(
        err.to_string(),
        "partition error: No supported filesystem found"
    );
}

#[test]
fn whole_device_iso9660() {
    let img = Img::new(MIB).iso(0).build();
    assert_eq!(
        list(&img).unwrap(),
        [part(0xFF, 0, MIB as u64, "Unknown", "ISO9660")]
    );
}

#[test]
fn small_iso9660() {
    // 0x8000 of system area, primary volume descriptor and terminator
    let img = Img::new(0x9000).iso(0).build();
    assert_eq!(
        list(&img).unwrap(),
        [part(0xFF, 0, 0x9000, "Unknown", "ISO9660")]
    );
}

#[test]
fn iso9660_second_descriptor() {
    // only the descriptor at 0x8800 is valid
    let mut img = Img::new(MIB);
    img.iso(0).put(0x8000, b"\x00\x00\x00\x00\x00\x00");
    assert_eq!(list(&img.build()).unwrap()[0].type_str, "ISO9660");
}

#[test]
fn empty_device() {
    let err = list(&Img::new(MIB).build()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "partition error: No supported filesystem found"
    );
}

#[test]
fn tiny_device() {
    // too small for the checks, but must not panic
    assert!(list(&[0; SECTOR]).is_err());
    assert!(list(&[]).is_err());
}

// Labels

#[test]
fn label_trimmed() {
    let img = Img::new(MIB).fat32(0, b"  A B\0\0\0\0\0\0").build();
    assert_eq!(list(&img).unwrap()[0].name_str, "A B");
    let img = Img::new(MIB).ext(0, 0x2C2, b"data\0\0\0").build();
    assert_eq!(list(&img).unwrap()[0].name_str, "data");
}

#[test]
fn label_empty_or_invalid() {
    let img = Img::new(MIB).fat32(0, b"           ").build();
    assert_eq!(list(&img).unwrap()[0].name_str, "Unknown");
    let img = Img::new(MIB).fat32(0, b"\0\0\0\0\0\0\0\0\0\0\0").build();
    assert_eq!(list(&img).unwrap()[0].name_str, "Unknown");
    let img = Img::new(MIB).fat32(0, b"\xff\xfeBAD      ").build();
    assert_eq!(list(&img).unwrap()[0].name_str, "Unknown");
}

// MBR

#[test]
fn mbr_partitions() {
    let mut img = Img::new(8 * MIB);
    img.mbr(&[
        (0x0c, 2048, 2048),
        (0x07, 4096, 2048),
        (0x83, 6144, 2048),
        (0x0e, 8192, 2048),
    ])
    .fat32(2048 * SECTOR, b"FIRST      ")
    .ntfs(4096 * SECTOR)
    .ext(6144 * SECTOR, 0x2C2, b"third")
    .fat16(8192 * SECTOR, b"FOURTH     ");
    assert_eq!(
        list(&img.build()).unwrap(),
        [
            part(0x0c, 2048, MIB as u64, "FIRST", "FAT"),
            part(0x07, 4096, MIB as u64, "Unknown", "NTFS"),
            part(0x83, 6144, MIB as u64, "third", "Linux/Ext"),
            part(0x0e, 8192, MIB as u64, "FOURTH", "FAT"),
        ]
    );
}

#[test]
fn mbr_unknown_types() {
    // the type code is replaced by the one of the detected file system
    let mut img = Img::new(4 * MIB);
    img.mbr(&[(0x42, 2048, 2048), (0xda, 4096, 2048)])
        .exfat(2048 * SECTOR);
    assert_eq!(
        list(&img.build()).unwrap(),
        [
            part(0x07, 2048, MIB as u64, "Unknown", "EXFAT"),
            part(0, 4096, MIB as u64, "Unknown", "Unknown"),
        ]
    );
}

#[test]
fn mbr_ext3_partition() {
    let mut img = Img::new(4 * MIB);
    img.mbr(&[(0x83, 2048, 2048), (0x0c, 4096, 2048)])
        .ext(2048 * SECTOR, 0x6, b"old")
        .fat32(4096 * SECTOR, b"NEW        ");
    let parts = list(&img.build()).unwrap();
    assert_eq!(parts[0], part(0, 2048, MIB as u64, "Unknown", "Unknown"));
    assert_eq!(parts[1].type_str, "FAT");
}

#[test]
fn mbr_single_unknown_partition_iso9660() {
    // e.g. an ISO9660 image in a partition: checked from the partition start
    let mut img = Img::new(4 * MIB);
    img.mbr(&[(0x17, 2048, 4096)]).iso(2048 * SECTOR);
    assert_eq!(
        list(&img.build()).unwrap(),
        [part(0xFF, 2048, 2 * MIB as u64, "Unknown", "ISO9660")]
    );
}

#[test]
fn mbr_single_partition_not_iso9660() {
    // ISO9660 descriptor at the start of the device, not in the partition
    let mut img = Img::new(4 * MIB);
    img.mbr(&[(0x17, 2048, 4096)]).iso(0);
    let err = list(&img.build()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "partition error: No supported filesystem found"
    );
}

#[test]
fn mbr_partition_out_of_device() {
    // a partition (bad table or truncated image) beyond the end of the device
    // doesn't prevent listing the others
    let mut img = Img::new(2 * MIB);
    img.mbr(&[(0x0c, 2048, 2048), (0x0c, 100_000, 2048)])
        .fat32(2048 * SECTOR, b"OK         ");
    assert_eq!(
        list(&img.build()).unwrap(),
        [
            part(0x0c, 2048, MIB as u64, "OK", "FAT"),
            part(0x0c, 100_000, MIB as u64, "Unknown", "Unknown"),
        ]
    );
}

#[test]
fn mbr_invalid_status_is_no_table() {
    // invalid status byte: not a partition table, e.g. FAT boot code
    let mut img = Img::new(MIB);
    img.fat32(0, b"NOTABLE    ")
        .put(446, &[0x12, 0, 0, 0, 0x0c]);
    assert_eq!(
        list(&img.build()).unwrap(),
        [part(0xb, 0, MIB as u64, "NOTABLE", "FAT")]
    );
}

// GPT

#[test]
fn gpt_partitions() {
    let mut img = Img::new(8 * MIB);
    img.gpt(SECTOR, &[(2048, 4095, "data"), (4096, 8191, "linux")])
        .fat32(2048 * SECTOR, b"VOL        ")
        .ext(4096 * SECTOR, 0x2C2, b"root");
    assert_eq!(
        list(&img.build()).unwrap(),
        [
            part(0xb, 2048, MIB as u64, "VOL", "FAT"),
            part(0x83, 4096, 2 * MIB as u64, "root", "Linux/Ext"),
        ]
    );
}

#[test]
fn gpt_name_kept_without_label() {
    let mut img = Img::new(4 * MIB);
    img.gpt(SECTOR, &[(2048, 4095, "My data"), (4096, 6143, "")])
        .ntfs(2048 * SECTOR)
        .exfat(4096 * SECTOR);
    assert_eq!(
        list(&img.build()).unwrap(),
        [
            part(0x7, 2048, MIB as u64, "My data", "NTFS"),
            part(0x7, 4096, MIB as u64, "Unknown", "EXFAT"),
        ]
    );
}

#[test]
fn gpt_bad_crc_is_no_table() {
    let mut img = Img::new(4 * MIB);
    img.gpt(SECTOR, &[(2048, 4095, "data")])
        .put(SECTOR + 0x10, &[0; 4]);
    // the protective MBR is all bootsector returns: no partition
    let err = list(&img.build()).unwrap_err();
    assert!(err.to_string().starts_with("partition error"), "{err}");
}
