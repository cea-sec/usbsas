//! Unit tests: the process runs in a thread from `WaitNewFile` (`Init` would
//! apply seccomp) on a temporary archive and is driven over pipes as usbsas
//! does. The archive is read back with the `tar` crate.

use super::*;
use std::{
    fs::File,
    io::{self, Read},
    os::fd::OwnedFd,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    thread::{self, JoinHandle},
};
use usbsas_comm::{ComRqWriteDst, Comm, ProtoReqCommon, ProtoReqWriteDst, SendRecv};
use usbsas_proto::writedst::{
    Request, RequestBitVec, RequestClose, RequestEndFile, RequestNewFile, RequestWriteFile,
    Response, response,
};

const TS: i64 = 1_600_000_000;

/// Temporary archive file, removed on drop.
struct TmpTar(PathBuf);

impl TmpTar {
    fn new() -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "usbsas-files2tar-test-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::SeqCst)
        ));
        File::create(&path).unwrap();
        TmpTar(path)
    }

    /// Paths and contents of the entries.
    fn entries(&self) -> Vec<(String, Vec<u8>)> {
        let mut archive = tar::Archive::new(File::open(&self.0).unwrap());
        archive
            .entries()
            .unwrap()
            .map(|e| {
                let mut e = e.unwrap();
                let mut data = Vec::new();
                e.read_to_end(&mut data).unwrap();
                (e.path().unwrap().to_string_lossy().into_owned(), data)
            })
            .collect()
    }

    fn paths(&self) -> Vec<String> {
        self.entries().into_iter().map(|(p, _)| p).collect()
    }
}

impl Drop for TmpTar {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn pipe_files() -> (File, File) {
    let (r, w) = io::pipe().unwrap();
    (File::from(OwnedFd::from(r)), File::from(OwnedFd::from(w)))
}

/// files2tar running in a thread, waiting for a new file.
struct Running {
    rq: ComRqWriteDst,
    handle: JoinHandle<Result<()>>,
    tar: TmpTar,
}

impl Running {
    fn spawn() -> Self {
        let tar = TmpTar::new();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&tar.0)
            .unwrap();
        let (rq_in, rp_out) = pipe_files();
        let (rp_in, rq_out) = pipe_files();
        // Built in the thread: the archive writer isn't Send
        let handle = thread::spawn(move || {
            let mut archive: Box<dyn ArchiveWriter> = Box::new(TarWriter::new(file));
            archive.init()?;
            Files2Tar {
                comm: Comm::new(rp_in, rp_out),
                state: State::WaitNewFile(WaitNewFileState { archive }),
            }
            .main_loop()
        });
        Running {
            rq: Comm::new(rq_in, rq_out),
            handle,
            tar,
        }
    }

    fn newfile(&mut self, path: &str, ftype: FileType, size: u64) -> io::Result<()> {
        self.rq
            .newfile(RequestNewFile {
                path: path.into(),
                size,
                ftype: ftype.into(),
                timestamp: TS,
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
        self.newfile(path, FileType::Regular, data.len() as u64)
            .unwrap();
        for (i, part) in data.chunks(chunk).enumerate() {
            self.writefile(path, (i * chunk) as u64, part).unwrap();
        }
        self.endfile(path).unwrap();
    }

    fn close(&mut self) {
        self.rq.close(RequestClose {}).unwrap();
    }

    fn recv(&mut self) -> response::Msg {
        let rep: Response = self.rq.recv().unwrap();
        rep.msg.unwrap()
    }

    /// Send `End` and wait for the process to exit, returns the archive.
    fn end(mut self) -> TmpTar {
        self.rq.end().unwrap();
        drop(self.rq);
        self.handle.join().unwrap().unwrap();
        self.tar
    }
}

/// Content larger than a chunk, not a repeated pattern.
fn content(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + i / 251) as u8).collect()
}

#[test]
fn transfer() {
    let big = content(3 * 100_000 + 17);
    let mut run = Running::spawn();
    run.newfile("/dir", FileType::Directory, 0).unwrap();
    run.endfile("/dir").unwrap();
    run.file("/dir/big.bin", &big, 100_000);
    run.file("/empty", b"", 4096);
    run.newfile("config.json", FileType::Metadata, 2).unwrap();
    run.writefile("config.json", 0, b"{}").unwrap();
    run.endfile("config.json").unwrap();
    run.close();
    let tar = run.end();
    assert_eq!(
        tar.entries(),
        [
            ("data/".into(), vec![]),
            ("data/dir".into(), vec![]),
            ("data/dir/big.bin".into(), big),
            ("data/empty".into(), vec![]),
            ("config.json".into(), b"{}".to_vec()),
        ]
    );
}

/// A directory announced with a size (as ext4 reports) has a header of size 0.
#[test]
fn directory_with_size() {
    let mut run = Running::spawn();
    run.newfile("/dir", FileType::Directory, 4096).unwrap();
    run.endfile("/dir").unwrap();
    run.file("/dir/a", b"a", 4096);
    run.close();
    assert_eq!(
        run.end().entries(),
        [
            ("data/".into(), vec![]),
            ("data/dir".into(), vec![]),
            ("data/dir/a".into(), b"a".to_vec()),
        ]
    );
}

/// Data after a directory header would be read as the next headers.
#[test]
fn directory_with_data_refused() {
    let mut run = Running::spawn();
    run.newfile("/dir", FileType::Directory, 4096).unwrap();
    assert!(run.writefile("/dir", 0, &[0; 512]).is_err());
    run.end();
}

#[test]
fn oversize_refused() {
    let mut run = Running::spawn();
    run.newfile("/a", FileType::Regular, 4).unwrap();
    run.writefile("/a", 0, b"0123").unwrap();
    let err = run.writefile("/a", 4, b"4").unwrap_err();
    assert!(err.to_string().contains("oversize"), "{err}");
    run.end();
}

/// Less data than announced: the next header and data would be read as the
/// end of this file, and file data as headers. The content of a later file
/// would inject entries the server never saw (at the root of the archive, or
/// filtered names).
#[test]
fn undersize_refused() {
    let mut run = Running::spawn();
    run.newfile("/a", FileType::Regular, 1024).unwrap();
    run.writefile("/a", 0, b"short").unwrap();
    let err = run.endfile("/a").unwrap_err();
    assert!(err.to_string().contains("undersize"), "{err}");
    run.end();
}

#[test]
fn undersize_no_data_refused() {
    let mut run = Running::spawn();
    run.newfile("/a", FileType::Regular, 512).unwrap();
    assert!(run.endfile("/a").is_err());
    run.end();
}

#[test]
fn bad_name_continues() {
    let mut run = Running::spawn();
    assert!(run.newfile("/../etc/passwd", FileType::Regular, 1).is_err());
    assert!(run.newfile("/", FileType::Regular, 0).is_err());
    // The process keeps going
    run.file("/ok", b"ok", 4096);
    run.close();
    assert_eq!(run.end().paths(), ["data/", "data/ok"]);
}

#[test]
fn bad_file_type() {
    let mut run = Running::spawn();
    assert!(run.newfile("/a", FileType::Other, 0).is_err());
    run.end();
}

#[test]
fn bad_request() {
    let mut run = Running::spawn();
    // A single error: the next response is the one of End
    assert!(run.rq.bitvec(RequestBitVec {}).is_err());
    run.end();
}

#[test]
fn bad_request_while_writing() {
    let mut run = Running::spawn();
    run.newfile("/a", FileType::Regular, 1).unwrap();
    assert!(run.rq.close(RequestClose {}).is_err());
    run.end();
}

#[test]
fn request_after_close() {
    let mut run = Running::spawn();
    run.close();
    run.rq
        .send(Request {
            msg: Some(Msg::NewFile(RequestNewFile::default())),
        })
        .unwrap();
    assert!(matches!(run.recv(), response::Msg::Error(_)));
    // The process exits after the error
    drop(run.rq);
    run.handle.join().unwrap().unwrap();
}

#[test]
fn end_without_close() {
    let mut run = Running::spawn();
    run.file("/a", b"a", 4096);
    // The archive isn't finished but the entries written are readable
    assert_eq!(run.end().paths(), ["data/", "data/a"]);
}

#[test]
fn process_ends_when_parent_leaves() {
    let run = Running::spawn();
    drop(run.rq);
    assert!(run.handle.join().unwrap().is_err());
}

#[test]
fn new_end() {
    let (rq_in, rp_out) = pipe_files();
    let (rp_in, rq_out) = pipe_files();
    let handle = thread::spawn(move || Files2Tar::new_end(Comm::new(rp_in, rp_out))?.main_loop());
    let mut rq: ComRqWriteDst = Comm::new(rq_in, rq_out);
    rq.end().unwrap();
    handle.join().unwrap().unwrap();
}
