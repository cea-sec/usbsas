//! Unit tests: the process runs in a thread from `WaitFsInfos` (`Init` would
//! apply seccomp) on a temporary image and is driven over pipes as usbsas does.
//! The written file systems are read back with the `usbsas-fsrw` readers.

use super::*;
use std::{
    fs::{self, File},
    io,
    os::fd::OwnedFd,
    path::PathBuf,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
};
use usbsas_comm::{ComRqWriteDst, Comm, ProtoReqCommon, ProtoReqWriteDst, SendRecv};
use usbsas_fsrw::FSRead;
use usbsas_proto::writedst::{
    Request, RequestBitVec, RequestClose, RequestEndFile, RequestInit, RequestNewFile,
    RequestWriteData, RequestWriteFile, RequestWriteRaw, Response, response,
};

const MIB: u64 = 1 << 20;
/// Size of the destination device
const DEV_SIZE: u64 = 32 * MIB;
/// Even timestamps: FAT and exFAT have a 2 seconds resolution
const TS_DIR: i64 = 1_600_000_000;
const TS_FILE: i64 = 1_650_000_000;

/// Temporary image file, removed on drop.
struct TmpImg(PathBuf, Option<MutexGuard<'static, ()>>);

impl TmpImg {
    fn new(size: u64) -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "usbsas-files2fs-test-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::SeqCst)
        ));
        File::create(&path).unwrap().set_len(size).unwrap();
        TmpImg(path, None)
    }

    fn open_rw(&self) -> File {
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.0)
            .unwrap()
    }

    fn read(&self) -> Vec<u8> {
        fs::read(&self.0).unwrap()
    }

    /// Mount the file system of the partition.
    fn mount(&self, fstype: FsType) -> Box<dyn FSRead<StreamSlice<File>>> {
        let slice = StreamSlice::new(
            File::open(&self.0).unwrap(),
            SECTOR_START * SECTOR_SIZE,
            DEV_SIZE,
        )
        .unwrap();
        match fstype {
            FsType::Fat => Box::new(fat::FatReader::new(slice, SECTOR_SIZE as u32).unwrap()),
            FsType::Exfat => Box::new(ff::ExFatFsReader::new(slice, SECTOR_SIZE as u32).unwrap()),
            FsType::Ntfs => Box::new(ntfs::NTFS::new(slice, SECTOR_SIZE as u32).unwrap()),
        }
    }
}

impl Drop for TmpImg {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn pipe_files() -> (File, File) {
    let (r, w) = io::pipe().unwrap();
    (File::from(OwnedFd::from(r)), File::from(OwnedFd::from(w)))
}

/// files2fs running in a thread, waiting for `Init` or `WriteRaw`.
struct Running {
    rq: ComRqWriteDst,
    handle: JoinHandle<Result<()>>,
    img: TmpImg,
}

/// The C FatFs used for exFAT mounts one volume per process (as files2fs
/// does), tests using it run one at a time: the image holds the lock until
/// dropped, after being written and read back.
fn exfat_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|err| err.into_inner())
}

impl Running {
    fn spawn(dev_size: u64) -> Self {
        let img = TmpImg::new(dev_size);
        let (rq_in, rp_out) = pipe_files();
        let (rp_in, rq_out) = pipe_files();
        let fs = img.open_rw();
        // Built in the thread: the file system writers aren't Send
        let handle = thread::spawn(move || {
            Files2Fs {
                comm: Comm::new(rp_in, rp_out),
                state: State::WaitFsInfos(WaitFsInfosState { fs }),
            }
            .main_loop()
        });
        Running {
            rq: Comm::new(rq_in, rq_out),
            handle,
            img,
        }
    }

    /// Spawned and initialized with a new `fstype` file system.
    fn mkfs(fstype: FsType) -> Self {
        let exfat = (fstype == FsType::Exfat).then(exfat_lock);
        let mut run = Self::spawn(DEV_SIZE);
        run.img.1 = exfat;
        run.init(DEV_SIZE, fstype).unwrap();
        run
    }

    fn init(&mut self, dev_size: u64, fstype: FsType) -> io::Result<()> {
        self.rq
            .init(RequestInit {
                dev_size,
                fstype: fstype.into(),
            })
            .map(|_| ())
    }

    fn newfile(&mut self, path: &str, ftype: FileType, timestamp: i64) -> io::Result<()> {
        self.rq
            .newfile(RequestNewFile {
                path: path.into(),
                size: 0,
                ftype: ftype.into(),
                timestamp,
            })
            .map(|_| ())
    }

    fn writefile(&mut self, path: &str, offset: u64, data: &[u8]) -> io::Result<()> {
        self.rq
            .writefile(RequestWriteFile {
                path: path.into(),
                offset,
                data: data.to_vec(),
            })
            .map(|_| ())
    }

    fn endfile(&mut self, path: &str) -> io::Result<()> {
        self.rq
            .endfile(RequestEndFile { path: path.into() })
            .map(|_| ())
    }

    /// Write a whole file in chunks of `chunk` bytes.
    fn file(&mut self, path: &str, data: &[u8], chunk: usize) {
        self.newfile(path, FileType::Regular, TS_FILE).unwrap();
        for (i, part) in data.chunks(chunk).enumerate() {
            self.writefile(path, (i * chunk) as u64, part).unwrap();
        }
        self.endfile(path).unwrap();
    }

    /// Close the file system and get the bit vector of written sectors.
    fn close(&mut self) -> FileBitVec {
        self.rq.close(RequestClose {}).unwrap();
        let mut bytes = Vec::new();
        loop {
            let rep = self.rq.bitvec(RequestBitVec {}).unwrap();
            bytes.extend(rep.chunk);
            if rep.last {
                break;
            }
        }
        FileBitVec::from_vec(bytes)
    }

    /// Next response, for requests answered by several messages.
    fn recv(&mut self) -> response::Msg {
        let rep: Response = self.rq.recv().unwrap();
        rep.msg.unwrap()
    }

    /// Send `End` and wait for the process to exit, returns the image.
    fn end(mut self) -> TmpImg {
        self.rq.end().unwrap();
        drop(self.rq);
        self.handle.join().unwrap().unwrap();
        self.img
    }
}

fn read_all(fs: &mut dyn FSRead<StreamSlice<File>>, path: &str) -> Vec<u8> {
    let (_, size, _) = fs.get_attr(path).unwrap();
    let mut buf = vec![0; size as usize];
    let n = fs.read_file(path, &mut buf, 0, size).unwrap();
    assert_eq!(n, size);
    buf
}

/// Content larger than a chunk, not a repeated pattern.
fn content(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + i / 251) as u8).collect()
}

// Complete transfers

fn transfer(fstype: FsType, ptype: u8) {
    let big = content(3 * 100_000 + 17);
    let mut run = Running::mkfs(fstype);
    run.newfile("/dir", FileType::Directory, TS_DIR).unwrap();
    run.newfile("/dir/sub", FileType::Directory, TS_DIR)
        .unwrap();
    run.file("/dir/sub/big.bin", &big, 100_000);
    run.file("/small.txt", b"hello usbsas", 4096);
    run.file("/empty", b"", 4096);
    // Generated files (report) are written like regular files
    run.newfile("/report.json", FileType::Metadata, TS_FILE)
        .unwrap();
    run.writefile("/report.json", 0, b"{}").unwrap();
    run.endfile("/report.json").unwrap();
    let bitvec = run.close();
    let img = run.end();

    // MBR: one partition starting at SECTOR_START, up to the end of the device
    let raw = img.read();
    assert_eq!(raw.len() as u64, DEV_SIZE);
    assert_eq!(&raw[510..512], &[0x55, 0xAA]);
    assert_eq!(raw[446 + 4], ptype);
    let start = u32::from_le_bytes(raw[446 + 8..446 + 12].try_into().unwrap());
    let count = u32::from_le_bytes(raw[446 + 12..446 + 16].try_into().unwrap());
    assert_eq!(u64::from(start), SECTOR_START);
    assert_eq!(u64::from(start + count) * SECTOR_SIZE, DEV_SIZE);

    // Bit vector: one bit per sector of the device, the MBR area is always
    // written, and a mostly empty file system leaves sectors untouched
    assert_eq!(bitvec.len() as u64, DEV_SIZE / SECTOR_SIZE);
    assert!(bitvec[..SECTOR_START as usize].all());
    assert!(bitvec.count_zeros() > 0);
    // Every non zero sector is in the bit vector, fs2dev only writes those
    for (i, sector) in raw.chunks(SECTOR_SIZE as usize).enumerate() {
        if sector.iter().any(|b| *b != 0) {
            assert!(bitvec[i], "sector {i} written but not in the bit vector");
        }
    }

    // Files read back
    let mut fs = img.mount(fstype);
    let mut root: Vec<String> = fs
        .read_dir("/")
        .unwrap()
        .into_iter()
        .map(|f| f.path)
        .collect();
    root.sort();
    assert_eq!(root, ["/dir", "/empty", "/report.json", "/small.txt"]);
    assert_eq!(fs.get_attr("/dir/sub").unwrap().0, FileType::Directory);
    assert_eq!(read_all(fs.as_mut(), "/dir/sub/big.bin"), big);
    assert_eq!(read_all(fs.as_mut(), "/small.txt"), b"hello usbsas");
    assert_eq!(read_all(fs.as_mut(), "/report.json"), b"{}");
    assert_eq!(
        fs.get_attr("/empty").unwrap(),
        (FileType::Regular, 0, TS_FILE)
    );
    assert_eq!(fs.get_attr("/dir/sub/big.bin").unwrap().2, TS_FILE);
}

#[test]
fn transfer_fat() {
    transfer(FsType::Fat, 0x0c);
}

#[test]
fn transfer_exfat() {
    transfer(FsType::Exfat, 0x07);
}

#[test]
fn transfer_ntfs() {
    transfer(FsType::Ntfs, 0x07);
}

#[test]
fn old_partition_table_erased() {
    // Leftovers of a previous use of the image
    let mut run = Running::spawn(DEV_SIZE);
    {
        let mut f = run.img.open_rw();
        f.seek(SeekFrom::Start(SECTOR_SIZE)).unwrap();
        f.write_all(b"EFI PART").unwrap();
    }
    run.init(DEV_SIZE, FsType::Fat).unwrap();
    let bitvec = run.close();
    run.end();
    // The whole area before the partition is written on the device, which
    // erases an old GPT header (sector 1) even though files2fs didn't write it
    assert!(bitvec[..SECTOR_START as usize].all());
}

// Errors

#[test]
fn init_bad_dev_size() {
    for dev_size in [DEV_SIZE + 1, SECTOR_START * SECTOR_SIZE - SECTOR_SIZE, 0] {
        let mut run = Running::spawn(DEV_SIZE);
        let err = run.init(dev_size, FsType::Fat).unwrap_err();
        assert!(
            err.to_string().starts_with("Error mkfs"),
            "{dev_size}: {err}"
        );
        run.end();
    }
}

#[test]
fn init_bad_fstype() {
    let mut run = Running::spawn(DEV_SIZE);
    let err = run
        .rq
        .init(RequestInit {
            dev_size: DEV_SIZE,
            fstype: 42,
        })
        .unwrap_err();
    assert!(err.to_string().starts_with("Error mkfs"), "{err}");
    run.end();
}

#[test]
fn sparse_write_removes_file() {
    let mut run = Running::mkfs(FsType::Fat);
    run.newfile("/a.txt", FileType::Regular, TS_FILE).unwrap();
    run.writefile("/a.txt", 0, b"0123").unwrap();
    let err = run.writefile("/a.txt", 10, b"4567").unwrap_err();
    assert!(err.to_string().contains("sparse write"), "{err}");
    // The process keeps going: another file can be written
    run.file("/b.txt", b"b", 4096);
    run.close();
    let img = run.end();

    let mut fs = img.mount(FsType::Fat);
    assert!(fs.get_attr("/a.txt").is_err());
    assert_eq!(read_all(fs.as_mut(), "/b.txt"), b"b");
}

#[test]
fn newdir_error_continues() {
    let mut run = Running::mkfs(FsType::Fat);
    // Parent doesn't exist
    assert!(
        run.newfile("/nope/dir", FileType::Directory, TS_DIR)
            .is_err()
    );
    run.newfile("/dir", FileType::Directory, TS_DIR).unwrap();
    run.close();
    let img = run.end();
    assert_eq!(
        img.mount(FsType::Fat).get_attr("/dir").unwrap().0,
        FileType::Directory
    );
}

#[test]
fn newfile_error_continues() {
    let mut run = Running::mkfs(FsType::Fat);
    // Parent doesn't exist: the file can't be created
    assert!(
        run.newfile("/nope/a.txt", FileType::Regular, TS_FILE)
            .is_err()
    );
    run.file("/b.txt", b"b", 4096);
    run.close();
    run.end();
}

#[test]
fn unexpected_request_while_writing_file() {
    let mut run = Running::mkfs(FsType::Fat);
    run.newfile("/a.txt", FileType::Regular, TS_FILE).unwrap();
    // Close in the middle of a file: error, then only End is accepted
    assert!(run.rq.close(RequestClose {}).is_err());
    run.end();
}

#[test]
fn end_without_close() {
    let mut run = Running::mkfs(FsType::Exfat);
    run.file("/a.txt", b"a", 4096);
    run.end();
}

#[test]
fn end_before_init() {
    Running::spawn(DEV_SIZE).end();
}

#[test]
fn bad_request_before_init() {
    let mut run = Running::spawn(DEV_SIZE);
    run.rq
        .send(Request {
            msg: Some(Msg::BitVec(RequestBitVec {})),
        })
        .unwrap();
    assert!(matches!(run.recv(), response::Msg::Error(_)));
    run.end();
}

#[test]
fn bad_file_type() {
    let mut run = Running::mkfs(FsType::Fat);
    assert!(run.newfile("/a", FileType::Other, TS_FILE).is_err());
    run.end();
}

#[test]
fn process_ends_when_parent_leaves() {
    let run = Running::mkfs(FsType::Fat);
    drop(run.rq);
    // Reading the next request fails: the process returns instead of hanging
    assert!(run.handle.join().unwrap().is_err());
}

// Raw writes (disk images)

#[test]
fn write_raw() {
    let mut run = Running::spawn(0);
    run.rq.writeraw(RequestWriteRaw {}).unwrap();
    let data = content(3 * 4096 + 5);
    for chunk in data.chunks(4096) {
        run.rq
            .writedata(RequestWriteData {
                data: chunk.to_vec(),
            })
            .unwrap();
    }
    let img = run.end();
    assert_eq!(img.read(), data);
}

#[test]
fn write_raw_bad_request() {
    let mut run = Running::spawn(0);
    run.rq.writeraw(RequestWriteRaw {}).unwrap();
    assert!(run.rq.close(RequestClose {}).is_err());
    run.end();
}
