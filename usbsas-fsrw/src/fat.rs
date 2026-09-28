//! FAT12/16/32 filesystem

use crate::{Error, Result};
use crate::{FSRead, FSWrite, WriteSeek};
use fscommon::BufStream;
use std::{
    cell::{Cell, RefCell},
    convert::TryFrom,
    io::{self, Read, Seek, SeekFrom, Write},
    rc::Rc,
};
use usbsas_proto::common::{FileInfo, FileType, FsType};

/// 1980-01-01T00:00:00Z, the FAT epoch.
const DOS_EPOCH: i64 = 315_532_800;

fn unix_ts_to_fat_datetime(ts: i64) -> fatfs::DateTime {
    let dt = time::OffsetDateTime::from_unix_timestamp(ts)
        .unwrap_or_else(|_| time::OffsetDateTime::from_unix_timestamp(DOS_EPOCH).unwrap());
    fatfs::DateTime {
        date: fatfs::Date {
            year: dt.year().clamp(1980, 2107) as u16,
            month: u16::from(u8::from(dt.month())),
            day: u16::from(dt.day()),
        },
        time: fatfs::Time {
            hour: u16::from(dt.hour()),
            min: u16::from(dt.minute()),
            sec: u16::from(dt.second()),
            millis: dt.millisecond(),
        },
    }
}

fn fat_datetime_to_unix_ts(dt: fatfs::DateTime) -> i64 {
    let date = time::Date::from_calendar_date(
        dt.date.year as i32,
        time::Month::try_from(dt.date.month as u8).unwrap_or(time::Month::January),
        dt.date.day as u8,
    )
    .unwrap_or_else(|_| time::Date::from_calendar_date(1980, time::Month::January, 1).unwrap());
    let time = time::Time::from_hms(dt.time.hour as u8, dt.time.min as u8, dt.time.sec as u8)
        .unwrap_or_else(|_| time::Time::from_hms(0, 0, 0).unwrap());
    time::PrimitiveDateTime::new(date, time)
        .assume_utc()
        .unix_timestamp()
}

thread_local! {
    static NEXT_TIMESTAMP: Cell<i64> = const { Cell::new(DOS_EPOCH) };
}

/// Time provider for Fat filesystem
#[derive(Debug)]
struct FatTimeProvider;

impl fatfs::TimeProvider for FatTimeProvider {
    fn get_current_date(&self) -> fatfs::Date {
        unix_ts_to_fat_datetime(NEXT_TIMESTAMP.with(Cell::get)).date
    }
    fn get_current_date_time(&self) -> fatfs::DateTime {
        unix_ts_to_fat_datetime(NEXT_TIMESTAMP.with(Cell::get))
    }
}

static TIME_PROVIDER: FatTimeProvider = FatTimeProvider;

fn with_next_timestamp<R>(timestamp: i64, f: impl FnOnce() -> R) -> R {
    NEXT_TIMESTAMP.with(|c| c.set(timestamp));
    f()
}

fn io_err(err: impl std::fmt::Display) -> io::Error {
    io::Error::other(err.to_string())
}

/// `fatfs::FileSystem<T>` requires `T: Read + Write + Seek` even when only
/// reading, and takes full ownership of `T` without ever giving it back. Both
/// wrappers below route IO through a `Rc<RefCell<Option<T>>>` so the inner
/// reader/writer can be reclaimed on `unmount_fs`, once the filesystem itself
/// has been dropped (same trick as `ext4fs.rs`).
struct SharedReadIo<T>(Rc<RefCell<Option<T>>>);

impl<T: Read> Read for SharedReadIo<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| io_err("inner reader gone"))?
            .read(buf)
    }
}

impl<T: Seek> Seek for SharedReadIo<T> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| io_err("inner reader gone"))?
            .seek(pos)
    }
}

// FatFs's T needs read + write + seek but write not really needed in our case
impl<T> Write for SharedReadIo<T> {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "read only"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct SharedWriteIo<T>(Rc<RefCell<Option<T>>>);

impl<T: Read> Read for SharedWriteIo<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| io_err("inner writer gone"))?
            .read(buf)
    }
}

impl<T: Write> Write for SharedWriteIo<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| io_err("inner writer gone"))?
            .write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| io_err("inner writer gone"))?
            .flush()
    }
}

impl<T: Seek> Seek for SharedWriteIo<T> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| io_err("inner writer gone"))?
            .seek(pos)
    }
}

/// Returns the directory at `path`, or the root directory
fn resolve_dir<'fs, T: fatfs::ReadWriteSeek>(
    fs: &'fs fatfs::FileSystem<T>,
    path: &str,
) -> io::Result<fatfs::Dir<'fs, T>> {
    if path.trim_matches('/').is_empty() {
        Ok(fs.root_dir())
    } else {
        fs.root_dir().open_dir(path)
    }
}

/// Finds the `DirEntry` for `name` in `dir`. `fatfs` doesn't expose a "stat a
/// single entry" call, only `open_file`/`open_dir` (which require knowing
/// the entry type upfront), so listing the parent and matching the name is
/// the only way to get a `DirEntry` (and its size/timestamps) for `get_attr`.
fn find_entry<'fs, T: fatfs::ReadWriteSeek>(
    dir: &fatfs::Dir<'fs, T>,
    name: &str,
) -> io::Result<fatfs::DirEntry<'fs, T>> {
    for entry in dir.iter() {
        let entry = entry?;
        if entry.file_name().eq_ignore_ascii_case(name) {
            return Ok(entry);
        }
    }
    Err(io_err("no such file or directory"))
}

fn attr_of(entry: &fatfs::DirEntry<impl fatfs::ReadWriteSeek>) -> (FileType, u64, i64) {
    let ftype = if entry.is_dir() {
        FileType::Directory
    } else {
        FileType::Regular
    };
    (
        ftype,
        entry.len(),
        fat_datetime_to_unix_ts(entry.modified()),
    )
}

pub struct FatReader<T: Read + Seek> {
    fs: fatfs::FileSystem<BufStream<SharedReadIo<T>>>,
    inner: Rc<RefCell<Option<T>>>,
}

impl<T: Read + Seek> FSRead<T> for FatReader<T> {
    fn new(mut reader: T, _sector_size: u32) -> Result<Self> {
        reader
            .seek(SeekFrom::Start(0))
            .map_err(|err| Error::FSError(format!("fatfs: couldn't seek to 0: {err}")))?;
        let inner = Rc::new(RefCell::new(Some(reader)));
        let io = SharedReadIo(Rc::clone(&inner));
        let fs = fatfs::FileSystem::new(BufStream::new(io), fatfs::FsOptions::new())
            .map_err(|err| Error::FSError(format!("fatfs: couldn't mount: {err}")))?;
        Ok(FatReader { fs, inner })
    }

    fn get_attr(&mut self, path: &str) -> Result<(FileType, u64, i64)> {
        log::trace!("get attr {path}");

        // can't stat root dir
        if path.is_empty() || path == "/" {
            return Ok((FileType::Directory, 0, 0));
        }

        let trimmed = path.trim_matches('/');
        let (parent, name) = match trimmed.rfind('/') {
            Some(idx) => (&trimmed[..idx], &trimmed[idx + 1..]),
            None => ("", trimmed),
        };

        let dir = resolve_dir(&self.fs, parent)
            .map_err(|err| Error::FSError(format!("fatfs: couldn't get attr for {path}: {err}")))?;
        let entry = find_entry(&dir, name)
            .map_err(|err| Error::FSError(format!("fatfs: couldn't get attr for {path}: {err}")))?;
        Ok(attr_of(&entry))
    }

    fn read_dir(&mut self, path: &str) -> Result<Vec<FileInfo>> {
        log::trace!("readdir {path}");
        let dir = resolve_dir(&self.fs, path)
            .map_err(|err| Error::FSError(format!("fatfs: couldn't read dir {path}: {err}")))?;
        let mut files_info = vec![];
        for entry in dir.iter() {
            let entry = entry
                .map_err(|err| Error::FSError(format!("fatfs: couldn't read dir {path}: {err}")))?;
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let (ftype, size, timestamp) = attr_of(&entry);
            files_info.push(FileInfo {
                path: format!("{}/{name}", path.trim_end_matches('/')),
                size,
                timestamp,
                ftype: ftype.into(),
            });
        }
        Ok(files_info)
    }

    fn read_file(
        &mut self,
        path: &str,
        buf: &mut Vec<u8>,
        offset: u64,
        bytes_to_read: u64,
    ) -> Result<u64> {
        log::trace!("read_file {path}");
        let mut file = self
            .fs
            .root_dir()
            .open_file(path)
            .map_err(|err| Error::FSError(format!("fatfs: couldn't open {path}: {err}")))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|err| Error::FSError(format!("fatfs: couldn't seek in {path}: {err}")))?;

        // read() while buffer isn't full or EOF is reached.
        // don't use read_exact() because it would ret an error and fuse always asks 4kb
        let mut bytes_read = 0;
        loop {
            match file.read(&mut buf[bytes_read..]) {
                Ok(size) => {
                    bytes_read += size;
                    if bytes_read as u64 == bytes_to_read || size == 0 {
                        return Ok(bytes_read as u64);
                    }
                }
                Err(err) => {
                    log::error!("fatfs: read error: {err}");
                    return Err(Error::FSError(format!(
                        "fatfs: couldn't read {path}: {err}"
                    )));
                }
            }
        }
    }

    fn unmount_fs(self: Box<Self>) -> Result<T> {
        log::trace!("unmount_fs");
        drop(self.fs);
        self.inner
            .borrow_mut()
            .take()
            .ok_or_else(|| Error::FSError("fatfs: couldn't get inner reader".into()))
    }
}

pub struct FatWriter<T: Read + Write + Seek> {
    fs: fatfs::FileSystem<BufStream<SharedWriteIo<T>>>,
    inner: Rc<RefCell<Option<T>>>,
}

impl<T: Read + Write + Seek> FSWrite<T> for FatWriter<T> {
    fn mkfs(
        mut writer: T,
        sector_size: u64,
        sector_count: u64,
        fstype: Option<FsType>,
    ) -> Result<Self>
    where
        Self: Sized,
    {
        match fstype {
            Some(FsType::Fat) => (),
            _ => return Err(Error::FSError("fatfs: unsupported fstype".into())),
        }
        let bytes_per_sector = u16::try_from(sector_size)?;
        let total_sectors = u32::try_from(sector_count)?;

        writer
            .seek(SeekFrom::Start(0))
            .map_err(|err| Error::FSError(format!("fatfs: couldn't seek to 0: {err}")))?;
        let inner = Rc::new(RefCell::new(Some(writer)));
        let io = SharedWriteIo(Rc::clone(&inner));
        let mut disk = BufStream::new(io);

        fatfs::format_volume(
            &mut disk,
            fatfs::FormatVolumeOptions::new()
                .bytes_per_sector(bytes_per_sector)
                .total_sectors(total_sectors),
        )
        .map_err(|err| Error::FSError(format!("fatfs: couldn't format volume: {err}")))?;
        disk.seek(SeekFrom::Start(0))
            .map_err(|err| Error::FSError(format!("fatfs: couldn't seek to 0: {err}")))?;

        let fs =
            fatfs::FileSystem::new(disk, fatfs::FsOptions::new().time_provider(&TIME_PROVIDER))
                .map_err(|err| Error::FSError(format!("fatfs: couldn't mount: {err}")))?;

        Ok(FatWriter { fs, inner })
    }

    fn newfile(&mut self, path: &str, timestamp: i64) -> Result<Box<dyn WriteSeek + '_>> {
        log::trace!("new file {path}");
        let file = with_next_timestamp(timestamp, || self.fs.root_dir().create_file(path))
            .map_err(|err| Error::FSError(format!("Couldn't create file {path}: {err}")))?;
        Ok(Box::new(file))
    }

    fn newdir(&mut self, path: &str, timestamp: i64) -> Result<()> {
        log::trace!("new dir: {path}");
        with_next_timestamp(timestamp, || self.fs.root_dir().create_dir(path))
            .map_err(|err| Error::FSError(format!("Couldn't create dir {path}: {err}")))?;
        Ok(())
    }

    fn removefile(&mut self, path: &str) -> Result<()> {
        log::trace!("rm file {path}");
        self.fs
            .root_dir()
            .remove(path)
            .map_err(|err| Error::FSError(format!("Couldn't rm file {path}: {err}")))
    }

    fn settimestamp(&mut self, _path: &str, _timestamp: i64) -> Result<()> {
        // Timestamps are handled by TimeProvider
        Ok(())
    }

    fn unmount_fs(self: Box<Self>) -> Result<T> {
        log::trace!("unmount_fs");
        drop(self.fs);
        self.inner
            .borrow_mut()
            .take()
            .ok_or_else(|| Error::FSError("fatfs: writer is gone".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn mkfs() -> FatWriter<Cursor<Vec<u8>>> {
        let disk = Cursor::new(vec![0u8; 16 * 1024 * 1024]);
        FatWriter::mkfs(disk, 512, 16 * 1024 * 1024 / 512, Some(FsType::Fat)).unwrap()
    }

    #[test]
    fn mkfs_rejects_non_fat() {
        let disk = Cursor::new(vec![0u8; 16 * 1024 * 1024]);
        assert!(
            FatWriter::mkfs(
                disk.clone(),
                512,
                16 * 1024 * 1024 / 512,
                Some(FsType::Exfat)
            )
            .is_err()
        );
        assert!(FatWriter::mkfs(disk, 512, 16 * 1024 * 1024 / 512, None).is_err());
    }

    #[test]
    fn roundtrip_read_write() {
        let mut writer = mkfs();
        writer.newdir("/quiche", 1_600_000_000).unwrap();
        {
            let mut f = writer.newfile("/quiche/tset.txt", 1_650_000_000).unwrap();
            f.write_all(b"This isn't gonna end well").unwrap();
        }
        {
            let mut f = writer.newfile("/usbsas.txt", 1_700_000_000).unwrap();
            f.write_all(b"random characters").unwrap();
        }
        writer
            .settimestamp("/quiche/tset.txt", 1_650_000_000)
            .unwrap();
        let disk = Box::new(writer).unmount_fs().unwrap();

        let mut reader = FatReader::new(disk, 512).unwrap();

        let root = reader.read_dir("/").unwrap();
        assert_eq!(root.len(), 2);

        let (ftype, size, ts) = reader.get_attr("/quiche/tset.txt").unwrap();
        assert_eq!(ftype, FileType::Regular);
        assert_eq!(size, 25);
        assert_eq!(ts, 1_650_000_000);

        let (ftype, _, _) = reader.get_attr("/quiche").unwrap();
        assert_eq!(ftype, FileType::Directory);

        let mut buf = vec![0u8; 64];
        let n = reader
            .read_file("/quiche/tset.txt", &mut buf, 0, 25)
            .unwrap();
        assert_eq!(&buf[..n as usize], b"This isn't gonna end well");

        let back = Box::new(reader).unmount_fs().unwrap();
        assert_eq!(back.get_ref().len(), 16 * 1024 * 1024);
    }

    #[test]
    fn remove_file() {
        let mut writer = mkfs();
        {
            let mut f = writer.newfile("/a.txt", 1_600_000_000).unwrap();
            f.write_all(b"bye").unwrap();
        }
        writer.removefile("/a.txt").unwrap();
        let disk = Box::new(writer).unmount_fs().unwrap();

        let mut reader = FatReader::new(disk, 512).unwrap();
        assert!(reader.get_attr("/a.txt").is_err());
        assert!(reader.read_dir("/").unwrap().is_empty());
    }
}
