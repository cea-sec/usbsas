//! Unit tests: the process runs in a thread from `LoadMetadata` (`Init` would
//! apply seccomp) on a temporary archive and is driven over pipes as usbsas
//! does. Archives are built with the `tar` crate, or crafted header by header
//! for what it refuses to write (the archive of a network source comes from a
//! remote server).

use super::*;
use std::{
    fs,
    io::{self, Write},
    os::fd::OwnedFd,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    thread::{self, JoinHandle},
};
use usbsas_comm::{ComRqFiles, Comm, ProtoReqCommon, ProtoReqFiles};
use usbsas_proto::files::{RequestGetAttr, RequestOpenDevice, RequestReadDir, RequestReadFile};

const BLOCK: usize = 512;
const TS: u64 = 1_600_000_000;

/// Archive built in memory.
struct Tar(Vec<u8>);

impl Tar {
    fn new() -> Self {
        Tar(Vec::new())
    }

    /// Entry with a raw `name` (not checked nor normalized) of `size` bytes,
    /// followed by `data` (padded to a block).
    fn raw(&mut self, name: &[u8], etype: tar::EntryType, size: u64, data: &[u8]) -> &mut Self {
        let mut header = tar::Header::new_ustar();
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.set_entry_type(etype);
        header.set_size(size);
        header.set_mode(0o644);
        header.set_mtime(TS);
        header.set_cksum();
        self.0.extend_from_slice(header.as_bytes());
        self.0.extend_from_slice(data);
        let pad = (BLOCK - data.len() % BLOCK) % BLOCK;
        self.0.extend(std::iter::repeat_n(0, pad));
        self
    }

    fn dir(&mut self, name: &str) -> &mut Self {
        self.raw(name.as_bytes(), tar::EntryType::Directory, 0, b"")
    }

    fn file(&mut self, name: &str, data: &[u8]) -> &mut Self {
        self.raw(
            name.as_bytes(),
            tar::EntryType::Regular,
            data.len() as u64,
            data,
        )
    }

    /// End of archive (2 empty blocks).
    fn build(&mut self) -> Vec<u8> {
        let mut tar = std::mem::take(&mut self.0);
        tar.extend_from_slice(&[0; 2 * BLOCK]);
        tar
    }
}

/// Archive as written by files2tar.
fn session_tar() -> Vec<u8> {
    Tar::new()
        .dir("data/")
        .dir("data/dir")
        .dir("data/dir/sub")
        .file("data/dir/sub/a.txt", b"hello")
        .file("data/big.bin", &content(30_000))
        .file("data/empty", b"")
        .file("config.json", b"{}")
        .build()
}

/// Content larger than a block, not a repeated pattern.
fn content(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + i / 251) as u8).collect()
}

/// Temporary archive file, removed on drop.
struct TmpTar(PathBuf);

impl TmpTar {
    fn new(data: &[u8]) -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "usbsas-tar2files-test-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::File::create(&path).unwrap().write_all(data).unwrap();
        TmpTar(path)
    }
}

impl Drop for TmpTar {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn pipe_files() -> (File, File) {
    let (r, w) = io::pipe().unwrap();
    (File::from(OwnedFd::from(r)), File::from(OwnedFd::from(w)))
}

/// tar2files running in a thread, loading the archive.
struct Running {
    rq: ComRqFiles,
    handle: JoinHandle<Result<()>>,
    _tar: TmpTar,
}

impl Running {
    fn spawn(tar: &[u8]) -> Self {
        let tmp = TmpTar::new(tar);
        let file = File::open(&tmp.0).unwrap();
        let (rq_in, rp_out) = pipe_files();
        let (rp_in, rq_out) = pipe_files();
        let tar2files = Tar2Files {
            comm: Comm::new(rp_in, rp_out),
            state: State::LoadMetadata(LoadMetadataState { tar: file }),
        };
        Running {
            rq: Comm::new(rq_in, rq_out),
            handle: thread::spawn(move || tar2files.main_loop()),
            _tar: tmp,
        }
    }

    fn getattr(&mut self, path: &str) -> io::Result<(FileType, u64, i64)> {
        let rep = self.rq.getattr(RequestGetAttr { path: path.into() })?;
        Ok((rep.ftype(), rep.size, rep.timestamp))
    }

    /// Sorted (path, type, size) of the entries of a directory.
    fn readdir(&mut self, path: &str) -> io::Result<Vec<(String, FileType, u64)>> {
        let mut entries: Vec<_> = self
            .rq
            .readdir(RequestReadDir { path: path.into() })?
            .filesinfo
            .into_iter()
            .map(|f| (f.path.clone(), f.ftype(), f.size))
            .collect();
        entries.sort();
        Ok(entries)
    }

    fn read(&mut self, path: &str, offset: u64, size: u64) -> io::Result<Vec<u8>> {
        Ok(self
            .rq
            .readfile(RequestReadFile {
                path: path.into(),
                offset,
                size,
            })?
            .data)
    }

    /// Paths of every entry, from the root.
    fn tree(&mut self) -> Vec<String> {
        let mut todo = vec![String::new()];
        let mut all = Vec::new();
        while let Some(dir) = todo.pop() {
            for (path, ftype, _) in self.readdir(&dir).unwrap() {
                assert!(all.len() < 1000, "loop in the tree: {all:?}");
                if ftype == FileType::Directory {
                    todo.push(path.clone());
                }
                all.push(path);
            }
        }
        all.sort();
        all
    }

    /// Send `End` and wait for the process to exit.
    fn end(mut self) {
        self.rq.end().unwrap();
        drop(self.rq);
        self.handle.join().unwrap().unwrap();
    }
}

// Archives written by files2tar

#[test]
fn session_archive() {
    let mut run = Running::spawn(&session_tar());
    assert_eq!(
        run.readdir("/").unwrap(),
        [
            ("big.bin".into(), FileType::Regular, 30_000),
            ("config.json".into(), FileType::Regular, 2),
            ("dir".into(), FileType::Directory, 0),
            ("empty".into(), FileType::Regular, 0),
        ]
    );
    assert_eq!(
        run.readdir("/dir").unwrap(),
        [("dir/sub".into(), FileType::Directory, 0)]
    );
    assert_eq!(
        run.getattr("/dir/sub/a.txt").unwrap(),
        (FileType::Regular, 5, TS as i64)
    );
    assert_eq!(run.read("/dir/sub/a.txt", 0, 5).unwrap(), b"hello");
    assert_eq!(run.read("/config.json", 0, 2).unwrap(), b"{}");
    assert_eq!(run.read("/empty", 0, 0).unwrap(), b"");
    // In chunks
    let big = content(30_000);
    let mut read = Vec::new();
    for offset in (0..30_000).step_by(7_000) {
        let size = (30_000 - offset).min(7_000);
        read.extend(run.read("/big.bin", offset, size).unwrap());
    }
    assert_eq!(read, big);
    run.end();
}

#[test]
fn paths_with_slashes() {
    let mut run = Running::spawn(&session_tar());
    assert!(run.getattr("dir/sub/a.txt").is_ok());
    assert!(run.getattr("/dir/sub/").is_ok());
    assert_eq!(run.readdir("dir/").unwrap().len(), 1);
    run.end();
}

#[test]
fn written_by_files2tar_builder() {
    // Long paths (GNU long name entries) as written by the tar crate
    let long = format!("data/{}/{}.txt", "d".repeat(150), "f".repeat(150));
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_ustar();
    header.set_size(4);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o644);
    builder
        .append_data(&mut header, &long, &b"long"[..])
        .unwrap();
    let mut run = Running::spawn(&builder.into_inner().unwrap());
    let path = long.strip_prefix("data/").unwrap();
    assert_eq!(run.read(path, 0, 4).unwrap(), b"long");
    run.end();
}

#[test]
fn missing_file() {
    let mut run = Running::spawn(&session_tar());
    assert!(run.getattr("/nope").is_err());
    assert!(run.read("/nope", 0, 1).is_err());
    assert!(run.readdir("/nope").unwrap().is_empty());
    // The process keeps going
    assert!(run.getattr("/empty").is_ok());
    run.end();
}

#[test]
fn read_max_size() {
    let mut run = Running::spawn(&session_tar());
    let err = run.read("/big.bin", 0, READ_FILE_MAX_SIZE + 1).unwrap_err();
    assert!(err.to_string().contains("max read size"), "{err}");
    run.end();
}

#[test]
fn bad_request() {
    let mut run = Running::spawn(&session_tar());
    // A single error: the next response is the one of End
    assert!(
        run.rq
            .opendevice(RequestOpenDevice {
                busnum: 0,
                devnum: 0
            })
            .is_err()
    );
    run.end();
}

#[test]
fn process_ends_when_parent_leaves() {
    let run = Running::spawn(&session_tar());
    drop(run.rq);
    assert!(run.handle.join().unwrap().is_err());
}

#[test]
fn locked_and_not_used() {
    let (rq_in, rp_out) = pipe_files();
    let (rp_in, rq_out) = pipe_files();
    let tar2files = Tar2Files {
        comm: Comm::new(rp_in, rp_out),
        state: State::WaitEnd(WaitEndState { err: None }),
    };
    let handle = thread::spawn(move || tar2files.main_loop());
    let mut rq: ComRqFiles = Comm::new(rq_in, rq_out);
    rq.end().unwrap();
    handle.join().unwrap().unwrap();
}

// Reads outside of a file

#[test]
fn read_past_end_of_file() {
    let mut run = Running::spawn(&session_tar());
    // a.txt is 5 bytes long: the following bytes are the next headers
    assert!(run.read("/dir/sub/a.txt", 0, 600).is_err());
    assert!(run.read("/dir/sub/a.txt", 3, 3).is_err());
    assert!(run.read("/dir/sub/a.txt", 5, 1).is_err());
    assert!(run.read("/dir/sub/a.txt", u64::MAX, 1).is_err());
    // Up to the end is fine
    assert_eq!(run.read("/dir/sub/a.txt", 3, 2).unwrap(), b"lo");
    assert_eq!(run.read("/dir/sub/a.txt", 5, 0).unwrap(), b"");
    run.end();
}

#[test]
fn read_directory() {
    let mut run = Running::spawn(&session_tar());
    assert!(run.read("/dir", 0, 512).is_err());
    run.end();
}

// Malicious archives (network source)

#[test]
fn root_not_listed() {
    let mut run = Running::spawn(&session_tar());
    // data/ itself isn't an entry of the root, but the root is a directory
    assert!(
        run.readdir("/")
            .unwrap()
            .iter()
            .all(|(p, _, _)| !p.is_empty())
    );
    assert_eq!(run.getattr("/").unwrap(), (FileType::Directory, 0, 0));
    assert_eq!(run.getattr("").unwrap(), (FileType::Directory, 0, 0));
    assert!(run.read("/", 0, 0).is_err());
    run.end();
    // Even without a data/ entry
    let mut run = Running::spawn(&Tar::new().file("data/a", b"a").build());
    assert_eq!(run.getattr("/").unwrap().0, FileType::Directory);
    run.end();
}

#[test]
fn bad_paths_ignored() {
    let tar = Tar::new()
        .dir("data/")
        .file("data/../escape", b"x")
        .file("../data/escape2", b"x")
        .file("data/a/../../escape3", b"x")
        .dir("data/a")
        .dir("data/a/..")
        .dir("data/.")
        .file("data/./dot", b"x")
        .file("data/a/./dot2", b"x")
        .file("data//double", b"x")
        .file("/data/abs", b"x")
        .file("data/ok", b"ok")
        .build();
    let mut run = Running::spawn(&tar);
    assert_eq!(run.tree(), ["a", "ok"]);
    for path in [
        "../escape",
        "a/../../escape3",
        "a/..",
        ".",
        "./dot",
        "a/./dot2",
        "/double",
    ] {
        assert!(run.getattr(path).is_err(), "{path} exposed");
    }
    run.end();
}

#[test]
fn only_data_and_report() {
    let tar = Tar::new()
        .file("other.txt", b"x")
        .file("datafile", b"x")
        .file("data", b"x")
        .file("dir/config.json", b"x")
        .file("data/ok", b"ok")
        .file("config.json", b"{}")
        .build();
    let mut run = Running::spawn(&tar);
    assert_eq!(run.tree(), ["config.json", "ok"]);
    run.end();
}

#[test]
fn other_entry_types_ignored() {
    let tar = Tar::new()
        .raw(b"data/link", tar::EntryType::Symlink, 0, b"")
        .raw(b"data/hard", tar::EntryType::Link, 0, b"")
        .raw(b"data/fifo", tar::EntryType::Fifo, 0, b"")
        .raw(b"data/char", tar::EntryType::Char, 0, b"")
        .file("data/ok", b"ok")
        .build();
    let mut run = Running::spawn(&tar);
    assert_eq!(run.tree(), ["ok"]);
    run.end();
}

#[test]
fn big_timestamp() {
    let mut tar = Tar::new();
    tar.file("data/a", b"a");
    // mtime above i64::MAX (GNU binary encoding)
    let mut header = tar::Header::new_gnu();
    header.set_path("data/b").unwrap();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(0);
    header.set_mtime(u64::MAX);
    header.set_cksum();
    tar.0.extend_from_slice(header.as_bytes());
    tar.file("data/c", b"c");
    let mut run = Running::spawn(&tar.build());
    // One bad timestamp doesn't hide the other files
    assert_eq!(run.tree(), ["a", "b", "c"]);
    assert_eq!(run.getattr("b").unwrap().2, i64::MAX);
    run.end();
}

#[test]
fn truncated_archive() {
    let mut tar = session_tar();
    // big.bin data cut
    tar.truncate(5 * BLOCK + 10_000);
    let mut run = Running::spawn(&tar);
    // The error answers the first request, not sent before it
    let err = run.readdir("/").unwrap_err();
    assert!(err.to_string().contains("archive"), "{err}");
    drop(run.rq);
    run.handle.join().unwrap().unwrap();
}

#[test]
fn garbage_archive() {
    let mut run = Running::spawn(&[0x42; 4 * BLOCK]);
    assert!(run.getattr("/a").is_err());
    drop(run.rq);
    run.handle.join().unwrap().unwrap();
}

#[test]
fn empty_archive() {
    let mut run = Running::spawn(b"");
    assert!(run.readdir("/").unwrap().is_empty());
    run.end();
}

#[test]
fn duplicate_entries() {
    // Like tar, the last entry wins
    let tar = Tar::new()
        .file("data/a", b"first")
        .file("data/a", b"second")
        .build();
    let mut run = Running::spawn(&tar);
    assert_eq!(run.readdir("/").unwrap().len(), 1);
    assert_eq!(run.read("a", 0, 6).unwrap(), b"second");
    run.end();
}
