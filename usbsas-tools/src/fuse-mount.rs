//! Very basic fuse impl to mount (read-only) usb devices with usbsas

use clap::{Arg, Command};
use std::{ffi::OsStr, path::Path, sync::RwLock, time::Duration};

use thiserror::Error;
use usbsas_comm::{protorequest, Comm};
use usbsas_process::{UsbsasChild, UsbsasChildSpawner};
use usbsas_proto as proto;

use fuse_mt::{
    CallbackResult, DirectoryEntry, RequestInfo, ResultEmpty, ResultEntry, ResultOpen,
    ResultReaddir, ResultSlice,
};

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("partition error: {0}")]
    Partition(String),
    #[error("privileges: {0}")]
    Privileges(#[from] usbsas_privileges::Error),
    #[error("privileges: {0}")]
    Process(#[from] usbsas_process::Error),
}
type Result<T> = std::result::Result<T, Error>;

protorequest!(
    CommFiles,
    files,
    opendevice = OpenDevice[RequestOpenDevice, ResponseOpenDevice],
    partitions = Partitions[RequestPartitions, ResponsePartitions],
    openpartition = OpenPartition[RequestOpenPartition, ResponseOpenPartition],
    getattr = GetAttr[RequestGetAttr, ResponseGetAttr],
    readdir = ReadDir[RequestReadDir, ResponseReadDir],
    readfile = ReadFile[RequestReadFile, ResponseReadFile],
    readsectors = ReadSectors[RequestReadSectors, ResponseReadSectors],
    end = End[RequestEnd, ResponseEnd]
);

const TTL: Duration = Duration::from_secs(1);

fn system_time_from_timestamp(ts: i64) -> std::time::SystemTime {
    let datetime =
        time::OffsetDateTime::from_unix_timestamp(ts).unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    datetime.into()
}

#[derive(Clone)]
struct Entry {
    size: u64,
    ftype: fuse_mt::FileType,
    ctime: i64,
}

impl From<&Entry> for fuse_mt::FileAttr {
    fn from(entry: &Entry) -> Self {
        let time = system_time_from_timestamp(entry.ctime);
        let perm = match entry.ftype {
            fuse_mt::FileType::Directory => 0o755,
            _ => 0o444,
        };
        fuse_mt::FileAttr {
            size: entry.size,
            blocks: 0,
            atime: time,
            mtime: time,
            ctime: time,
            crtime: time,
            kind: entry.ftype,
            perm,
            nlink: 2,
            uid: users::get_current_uid(),
            gid: users::get_current_gid(),
            rdev: 0,
            flags: 0,
        }
    }
}

struct UsbsasFS {
    scsi2files: RwLock<UsbsasChild<proto::files::Request>>,
}

impl UsbsasFS {
    fn new(busnum: u32, devnum: u32, partnum: u32) -> Result<Self> {
        log::debug!("Opening device {} {}", busnum, devnum);
        let mut scsi2files = UsbsasChildSpawner::new()
            .spawn::<usbsas_scsi2files::Scsi2Files, proto::files::Request>()?;
        let _ = scsi2files
            .comm
            .opendevice(proto::files::RequestOpenDevice { busnum, devnum })?;
        let parts = scsi2files
            .comm
            .partitions(proto::files::RequestPartitions {})?;
        if partnum as usize >= parts.partitions.len() {
            log::error!("Couldn't open part number {}", partnum);
            return Err(Error::Partition("Partition not found".into()));
        }
        log::debug!("Opening partition {}", partnum);
        if let Err(err) = scsi2files
            .comm
            .openpartition(proto::files::RequestOpenPartition { index: partnum })
        {
            return Err(Error::Partition(format!(
                "Couldn't open part number {} ({})",
                partnum, err
            )));
        }
        Ok(Self {
            scsi2files: RwLock::new(scsi2files),
        })
    }
}

impl Drop for UsbsasFS {
    fn drop(&mut self) {
        if let Err(err) = self
            .scsi2files
            .get_mut()
            .unwrap()
            .comm
            .end(proto::files::RequestEnd {})
        {
            log::error!("Couldn't end scsi2files: {}", err);
        };
    }
}

impl fuse_mt::FilesystemMT for UsbsasFS {
    fn getattr(&self, _req: RequestInfo, path: &Path, _fh: Option<u64>) -> ResultEntry {
        log::trace!("getattr: {:?}", path);
        let path_str = path.to_string_lossy().to_string();

        if path_str == "/" {
            let entry = Entry {
                size: 0,
                ftype: fuse_mt::FileType::Directory,
                ctime: 0,
            };
            return Ok((TTL, fuse_mt::FileAttr::from(&entry)));
        }

        let mut scsi2files = self.scsi2files.write().unwrap();

        let rep = match scsi2files
            .comm
            .getattr(proto::files::RequestGetAttr { path: path_str })
        {
            Ok(rep) => rep,
            Err(err) => {
                log::error!("getattr err: {:?} {}", &path, err);
                return Err(libc::ENOENT);
            }
        };
        let ftype = match usbsas_proto::common::FileType::from_i32(rep.ftype) {
            Some(usbsas_proto::common::FileType::Directory) => fuse_mt::FileType::Directory,
            _ => fuse_mt::FileType::RegularFile,
        };
        let entry = Entry {
            size: rep.size,
            ftype,
            ctime: rep.timestamp,
        };
        Ok((TTL, fuse_mt::FileAttr::from(&entry)))
    }

    fn read(
        &self,
        _req: RequestInfo,
        path: &Path,
        _fh: u64,
        offset: u64,
        size: u32,
        callback: impl FnOnce(ResultSlice<'_>) -> CallbackResult,
    ) -> CallbackResult {
        log::debug!("read: {:?} ({}/{})", &path, offset, size);

        match self
            .scsi2files
            .write()
            .unwrap()
            .comm
            .readfile(proto::files::RequestReadFile {
                path: path.to_string_lossy().to_string(),
                size: size as u64,
                offset,
            }) {
            Ok(rep) => callback(Ok(&rep.data)),
            Err(err) => {
                log::error!("read error {:?} ({}/{}): {:?}", &path, offset, size, err);
                callback(Err(libc::EIO))
            }
        }
    }

    fn opendir(&self, _req: RequestInfo, path: &Path, _flags: u32) -> ResultOpen {
        log::trace!("opendir: {:?}", path);
        Ok((0, 0))
    }

    fn readdir(&self, _req: RequestInfo, path: &Path, _fh: u64) -> ResultReaddir {
        log::trace!("readdir: {:?}", path);
        let dir_str = path.to_string_lossy().to_string();

        let mut scsi2files = self.scsi2files.write().unwrap();

        let mut result_entries: Vec<DirectoryEntry> = vec![];
        let rep = scsi2files
            .comm
            .readdir(proto::files::RequestReadDir {
                path: dir_str.trim_start_matches('/').to_string(),
            })
            .unwrap();
        for attrs in rep.filesinfo {
            let ftype = match usbsas_proto::common::FileType::from_i32(attrs.ftype) {
                Some(usbsas_proto::common::FileType::Directory) => fuse_mt::FileType::Directory,
                _ => fuse_mt::FileType::RegularFile,
            };
            result_entries.push(DirectoryEntry {
                name: attrs
                    .path
                    .trim_start_matches(&dir_str.trim_start_matches('/'))
                    .trim_start_matches('/')
                    .into(),
                kind: ftype,
            });
        }
        Ok(result_entries)
    }

    fn releasedir(&self, _req: RequestInfo, path: &Path, _fh: u64, _flags: u32) -> ResultEmpty {
        log::trace!("releasedir: {:?}", &path);
        Ok(())
    }
}

fn main() -> Result<()> {
    env_logger::builder().format_timestamp(None).init();

    let matches = Command::new("usbsas-fuse-mount")
        .about("Mount a (fuse) filesystem with usbsas")
        .version("1.0")
        .arg(
            Arg::new("busnum")
                .index(1)
                .required(true)
                .help("Bus nummber of the device to mount")
                .takes_value(true),
        )
        .arg(
            Arg::new("devnum")
                .index(2)
                .required(true)
                .help("Dev number of the device to mount")
                .takes_value(true),
        )
        .arg(
            Arg::new("mountpoint")
                .index(3)
                .required(true)
                .help("Path to mount the device")
                .takes_value(true),
        )
        .arg(
            Arg::new("part-num")
                .short('n')
                .long("part-num")
                .value_name("PARTNUM")
                .help("Partition number to mount")
                .default_value("1")
                .takes_value(true),
        )
        .get_matches();

    let mountpoint = matches.get_one::<String>("mountpoint").unwrap();

    let (busnum, devnum, partnum) = match (
        matches.get_one::<u32>("busnum"),
        matches.get_one::<u32>("devnum"),
        matches.get_one::<u32>("part-num"),
    ) {
        (Some(busnum), Some(devnum), Some(partnum)) => {
            (busnum.to_owned(), devnum.to_owned(), partnum.to_owned())
        }
        _ => {
            log::error!("Busnum / devnum / partnum must be u32");
            std::process::exit(1);
        }
    };

    // indexes are from 0
    let mut partnum = partnum;
    if partnum > 0 {
        partnum -= 1;
    }

    let fuse_options: Vec<&OsStr> = vec![OsStr::new("-o"), OsStr::new("ro,nodev,noexec,nosuid")];

    let usbsas_fs = UsbsasFS::new(busnum, devnum, partnum)?;
    fuse_mt::mount(
        fuse_mt::FuseMT::new(usbsas_fs, 0),
        &mountpoint,
        &fuse_options,
    )
    .unwrap();

    Ok(())
}
