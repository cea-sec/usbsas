//! FatFs bindings
//!
//! This crate can be used to read an already existing FAT file system or to
//! write a new one. Modifying an already existing file system is not supported.
//!
//! The struct `FatFs<T>` is a wrapper around ff's `FATFS` struct. It implements
//! reading a file system when `T` is `Read` + `Seek`, and writing a file system
//! when `T` is `Read` + `Write` + `Seek`.
//!
//! ff is built with a single volume (`FF_VOLUMES = 1`) and without locking
//! (`FF_FS_REENTRANT = 0`): it keeps a global pointer to the mounted `FATFS`.
//! Hence only one `FatFs` can exist at a time in a process; creating a second
//! one returns an error.

use std::{
    io::{self, Read, Seek, SeekFrom, Write},
    marker::PhantomData,
    os::raw::c_void,
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
};

mod ff_c;
mod ff_extern;
mod wrapper;
use wrapper::{WrapperFatFs, WrapperRead, WrapperReadWrite};

pub use ff_c::{FM_EXFAT, FM_FAT32};

const DRIVE: &str = "0:";

/// Convert a path to a NUL terminated UTF-16 string for ff. Paths come from
/// untrusted input: an embedded NUL would silently truncate the path on the C
/// side ("a\0b" would become "a"), so reject it.
fn str_to_utf16(string: &str) -> io::Result<Vec<u16>> {
    if string.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("ff invalid path: {string:?}"),
        ));
    }
    let mut ret: Vec<u16> = string.encode_utf16().collect();
    ret.push(0);
    Ok(ret)
}

/// `DRIVE` as a NUL terminated UTF-16 string
fn drive_utf16() -> Vec<u16> {
    DRIVE.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Earliest date representable in FAT: 1980-01-01 00:00:00 UTC
const FAT_TIMESTAMP_MIN: i64 = 315_532_800;
/// Latest date representable in FAT: 2107-12-31 23:59:58 UTC
const FAT_TIMESTAMP_MAX: i64 = 4_354_819_198;

/// Convert a UNIX timestamp to FAT (date, time) fields. Timestamps come from
/// untrusted input so they are clamped to the range FAT can represent instead
/// of overflowing the 7 bits year field (or underflowing before 1980).
fn timestamp_to_fat(timestamp: i64) -> (u16, u16) {
    let timestamp = timestamp.clamp(FAT_TIMESTAMP_MIN, FAT_TIMESTAMP_MAX);
    // Can't fail: the clamped range is well within time's supported range
    let dt = time::OffsetDateTime::from_unix_timestamp(timestamp)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    // year is in 1980..=2107 so year - 1980 fits in 7 bits
    let year = (dt.year() - 1980).clamp(0, 0x7F) as u16;
    let date = (year << 9) | (u16::from(u8::from(dt.month())) << 5) | u16::from(dt.day());
    let time =
        (u16::from(dt.hour()) << 11) | (u16::from(dt.minute()) << 5) | (u16::from(dt.second()) / 2);
    (date, time)
}

/// Returns unix timestamp from FILINFO struct without failing
/// (returns 01/01/1980 00:00:00 if parsing failed)
fn fno_to_timestamp(fno: &ff_c::FILINFO) -> i64 {
    time::PrimitiveDateTime::new(
        time::Date::from_calendar_date(
            ((fno.fdate >> 9) & 0x7F) as i32 + 1980,
            time::Month::try_from(((fno.fdate >> 5) & 0x0F) as u8).unwrap_or(time::Month::January),
            (fno.fdate & 0x1F) as u8,
        )
        .unwrap_or_else(|_| time::Date::from_calendar_date(1980, time::Month::January, 1).unwrap()),
        time::Time::from_hms(
            ((fno.ftime >> 11) & 0x1F) as u8,
            ((fno.ftime >> 5) & 0x3F) as u8,
            ((fno.ftime & 0x1F) * 2) as u8,
        )
        .unwrap_or_else(|_| time::Time::from_hms(0, 0, 0).unwrap()),
    )
    .assume_utc()
    .unix_timestamp()
}

fn check_sector_size(sector_size: u32) -> io::Result<()> {
    if (ff_c::FF_MIN_SS..=ff_c::FF_MAX_SS).contains(&sector_size) && sector_size.is_power_of_two() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("ff unsupported sector size: {sector_size}"),
        ))
    }
}

/// Set while a `FatFs` exists, ff's global volume table only has one slot.
static MOUNTED: AtomicBool = AtomicBool::new(false);

/// Ownership of ff's single volume slot, released on drop.
struct MountGuard(());

impl MountGuard {
    fn acquire() -> io::Result<Self> {
        MOUNTED
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| MountGuard(()))
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::ResourceBusy,
                    "ff: a FatFs is already mounted in this process",
                )
            })
    }
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        MOUNTED.store(false, Ordering::Release);
    }
}

/// Concrete type behind the `dyn WrapperFatFs`, needed to get `T` back.
#[derive(Clone, Copy, PartialEq, Eq)]
enum IoKind {
    Read,
    ReadWrite,
}

/// Wrapper around ff's FATFS struct
pub struct FatFs<T> {
    // Both pointers are owned by this struct (created with Box::into_raw and
    // freed in release()). They are kept as raw pointers rather than Boxes
    // because ff keeps aliases to them (the global volume table points to
    // `fs`, `fs.pdrv` points to `io`) and mutates them during calls made
    // through `&self`: a Box would assert unique ownership and forbid that.
    //
    // `io` points to a `Box<dyn WrapperFatFs + '_>` (a fat pointer, hence the
    // second Box to get a thin pointer for C's `void*`), whose concrete type
    // is given by `kind`. It is null once released.
    fs: NonNull<ff_c::FATFS>,
    io: *mut c_void,
    kind: IoKind,
    // Dropped after Drop::drop() has unmounted.
    _guard: MountGuard,
    _inner: PhantomData<T>,
}

impl<T> FatFs<T> {
    /// Allocate ff's state and take the global volume slot. Nothing is
    /// mounted yet but dropping the result is always safe.
    fn alloc(io: Box<dyn WrapperFatFs + '_>, kind: IoKind) -> io::Result<Self> {
        let guard = MountGuard::acquire()?;
        let io = Box::into_raw(Box::new(io)) as *mut c_void;
        let mut fs = Box::new(ff_c::FATFS::new());
        fs.pdrv = io;
        Ok(FatFs {
            fs: NonNull::from(Box::leak(fs)),
            io,
            kind,
            _guard: guard,
            _inner: PhantomData,
        })
    }

    fn mount(&mut self) -> io::Result<()> {
        let drive = drive_utf16();
        if unsafe { ff_c::f_mount(self.fs.as_ptr(), drive.as_ptr(), 1) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::other("ff f_mount error"));
        }
        Ok(())
    }

    /// Unmount, free ff's state and give back T along with the result of the
    /// final flush (for writable file systems). Returns None if already
    /// released.
    fn release(&mut self) -> Option<(T, io::Result<()>)> {
        let io = std::mem::replace(&mut self.io, std::ptr::null_mut());
        if io.is_null() {
            return None;
        }
        // Unregister fs from ff's global table before freeing what it points
        // to. f_mount(NULL) only fails on an invalid drive name. This is a
        // no-op if the mount failed before registering, and can't unregister
        // somebody else's volume since MountGuard makes us the only one.
        let drive = drive_utf16();
        if unsafe { ff_c::f_mount(std::ptr::null_mut(), drive.as_ptr(), 0) } != ff_c::FRESULT_FR_OK
        {
            eprintln!("f_mount (unmount) error");
        }
        // SAFETY: both pointers come from Box::into_raw in alloc(), are
        // released only once (io is now null) and ff doesn't reference them
        // anymore. `io` is a `Box<dyn WrapperFatFs>` whose concrete type is
        // given by `kind`, so the thin pointer casts below are to the real
        // type and Box::from_raw deallocates with the original layout.
        unsafe {
            drop(Box::from_raw(self.fs.as_ptr()));
            let outer: Box<Box<dyn WrapperFatFs + '_>> =
                Box::from_raw(io as *mut Box<dyn WrapperFatFs>);
            let mut wrapper: Box<dyn WrapperFatFs + '_> = *outer;
            let flushed = match self.kind {
                IoKind::Read => Ok(()),
                IoKind::ReadWrite => wrapper.flush(),
            };
            let raw = Box::into_raw(wrapper);
            let inner = match self.kind {
                IoKind::Read => Box::from_raw(raw as *mut WrapperRead<T>).into_inner(),
                IoKind::ReadWrite => Box::from_raw(raw as *mut WrapperReadWrite<T>).into_inner(),
            };
            Some((inner, flushed))
        }
    }

    /// Unmount fs and return T (flushed first if the file system is writable)
    pub fn into_inner(mut self) -> Result<T, io::Error> {
        let (inner, flushed) = self
            .release()
            .ok_or_else(|| io::Error::other("ff: FatFs already released"))?;
        flushed.map(|()| inner)
    }
}

/// Unmount and free everything if `into_inner()` wasn't called
impl<T> Drop for FatFs<T> {
    fn drop(&mut self) {
        if let Some((_, Err(err))) = self.release() {
            eprintln!("ff flush on drop failed: {err}");
        }
    }
}

impl<T: Read + Seek> FatFs<T> {
    /// Parse a FAT filesystem from T
    pub fn new(inner: T, sector_size: u32) -> Result<Self, io::Error> {
        check_sector_size(sector_size)?;
        let mut fs = Self::alloc(Box::new(WrapperRead::new(inner, sector_size)), IoKind::Read)?;
        // On error, fs is dropped which unmounts and frees everything
        fs.mount()?;
        Ok(fs)
    }

    /// Get file attributes from path
    pub fn get_attr(&self, path: &str) -> Result<FileInfo, io::Error> {
        let path_u16 = str_to_utf16(path)?;
        let mut fno = ff_c::FILINFO::new();
        if unsafe { ff_c::f_stat(path_u16.as_ptr(), &mut fno) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::other("ff f_atrib error"));
        }

        let timestamp = fno_to_timestamp(&fno);
        let ftype = if (u32::from(fno.fattrib) & ff_c::AM_DIR) > 0 {
            FileType::DIRECTORY
        } else {
            FileType::REGULAR
        };

        Ok(FileInfo {
            name: path.into(),
            size: fno.fsize,
            timestamp,
            ftype,
        })
    }

    /// Read directory from path
    pub fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, io::Error> {
        let mut dir = ff_c::DIR::new();
        let path_u16 = str_to_utf16(path)?;

        if unsafe { ff_c::f_opendir(&mut dir, path_u16.as_ptr()) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::other("ff f_opendir error"));
        }
        let ret = Self::read_dir_entries(&mut dir);
        unsafe { ff_c::f_closedir(&mut dir) };
        ret
    }

    fn read_dir_entries(dir: &mut ff_c::DIR) -> Result<Vec<FileInfo>, io::Error> {
        let mut files_info = Vec::new();
        loop {
            let mut fno = ff_c::FILINFO::new();

            if unsafe { ff_c::f_readdir(dir, &mut fno) } != ff_c::FRESULT_FR_OK {
                return Err(io::Error::other("ff f_readir error"));
            }

            if fno.fname[0] == 0 {
                break;
            }

            let idx_end = fno
                .fname
                .as_slice()
                .iter()
                .position(|&x| x == 0)
                .unwrap_or(fno.fname.len() - 1);
            let name = match String::from_utf16(&fno.fname[0..idx_end]) {
                Ok(name) => name,
                Err(_) => return Err(io::Error::other("ff from_utf16 error")),
            };

            let ftype = if (u32::from(fno.fattrib) & ff_c::AM_DIR) > 0 {
                FileType::DIRECTORY
            } else {
                FileType::REGULAR
            };

            files_info.push(FileInfo {
                name,
                size: fno.fsize,
                timestamp: fno_to_timestamp(&fno),
                ftype,
            });
        }
        Ok(files_info)
    }

    /// Read at most `bytes_to_read` (bounded by `buf.len()`) at `offset` from
    /// file in `path `into `buf`. Returns the number of bytes read.
    pub fn read_file(
        &self,
        path: &str,
        buf: &mut [u8],
        offset: u64,
        bytes_to_read: u64,
    ) -> Result<u64, io::Error> {
        let mut fp = ff_c::FIL::new();
        let mut bytes_read: ff_c::UINT = 0;
        let path_u16 = str_to_utf16(path)?;
        let len = usize::try_from(bytes_to_read)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let len = ff_c::UINT::try_from(len).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "ff read_file: size too big")
        })?;

        if unsafe { ff_c::f_open(&mut fp, path_u16.as_ptr(), ff_c::FA_READ as u8) }
            != ff_c::FRESULT_FR_OK
        {
            return Err(io::Error::other("ff f_open error"));
        }
        // closed on drop
        let mut file = FatFile::<T>::new(fp);
        if unsafe { ff_c::f_lseek(&mut file.inner, offset) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::other("ff f_lseek error"));
        }
        if unsafe {
            ff_c::f_read(
                &mut file.inner,
                buf.as_mut_ptr() as *mut c_void,
                len,
                &mut bytes_read,
            )
        } != ff_c::FRESULT_FR_OK
        {
            Err(io::Error::other("ff f_read error"))
        } else {
            Ok(u64::from(bytes_read))
        }
    }
}

impl<T: Read + Write + Seek> FatFs<T> {
    /// Create a new (ex)FAT filesystem on T and mount it
    pub fn mkfs(
        inner: T,
        sector_size: u32,
        sector_count: u32,
        fstype: u8,
    ) -> Result<Self, io::Error> {
        check_sector_size(sector_size)?;
        let mut fs = Self::alloc(
            Box::new(WrapperReadWrite::new(inner, sector_size)),
            IoKind::ReadWrite,
        )?;
        let drive = drive_utf16();

        let mkfs_params = ff_c::MKFS_PARM {
            // use S(uper)F(loppy)D(isk) param format to write fs directly
            // without already handled MBR / partition entry
            fmt: fstype | ff_c::FM_SFD as u8,
            n_fat: 0,
            align: 4096 / sector_size,
            n_root: 0,
            au_size: 0,
            sz_vol: sector_count,
        };
        // On error, fs is dropped which unmounts and frees everything
        if unsafe {
            ff_c::f_mkfs(
                fs.io,
                drive.as_ptr(),
                &mkfs_params,
                std::ptr::null_mut(),
                4096,
            )
        } != ff_c::FRESULT_FR_OK
        {
            return Err(io::Error::other("ff f_mkfs error"));
        }
        fs.mount()?;
        Ok(fs)
    }

    /// Create a new file on `path`. Returns a `Write`able `FatFile`.
    pub fn new_file<'a>(&'a mut self, path: &str) -> Result<FatFile<'a, T>, io::Error> {
        let fname = str_to_utf16(path)?;
        let mut fp = ff_c::FIL::new();
        if unsafe {
            ff_c::f_open(
                &mut fp,
                fname.as_ptr(),
                (ff_c::FA_WRITE | ff_c::FA_CREATE_NEW) as u8,
            )
        } != ff_c::FRESULT_FR_OK
        {
            return Err(io::Error::other("ff f_open error"));
        }
        Ok(FatFile::new(fp))
    }

    /// Create a new directory on `path`
    pub fn new_dir(&mut self, path: &str) -> Result<(), io::Error> {
        let dname = str_to_utf16(path)?;
        match unsafe { ff_c::f_mkdir(dname.as_ptr()) } {
            ff_c::FRESULT_FR_OK | ff_c::FRESULT_FR_EXIST => Ok(()),
            _ => Err(io::Error::other("ff f_mkdir error")),
        }
    }

    /// Remove file on `path`
    pub fn remove_file(&mut self, path: &str) -> Result<(), io::Error> {
        let fname = str_to_utf16(path)?;
        if unsafe { ff_c::f_unlink(fname.as_ptr()) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::other("ff f_mkdir error"));
        }
        Ok(())
    }

    /// Set UNIX timestamp on file `path`
    pub fn set_timestamp(&mut self, path: &str, timestamp: i64) -> Result<(), io::Error> {
        let fname = str_to_utf16(path)?;
        let mut fno = ff_c::FILINFO::new();
        (fno.fdate, fno.ftime) = timestamp_to_fat(timestamp);
        if unsafe { ff_c::f_utime(fname.as_ptr(), &fno) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::other("ff f_utime error"));
        }
        Ok(())
    }
}

/// Wrapper around an opened ff `FIL` struct. The lifetime is bound to the
/// underlying file system `FatFs<T>`.
pub struct FatFile<'a, T> {
    inner: ff_c::FIL,
    // Lifetimed ref to underlying FatFs so we don't outlive it
    _fsref: PhantomData<&'a FatFs<T>>,
}

impl<T> FatFile<'_, T> {
    /// Wrap a successfully opened `FIL` (it will be closed on drop).
    fn new(inner: ff_c::FIL) -> Self {
        FatFile {
            inner,
            _fsref: PhantomData,
        }
    }
}

/// Type of a file.
pub enum FileType {
    /// Regular file
    REGULAR,
    /// Directory
    DIRECTORY,
}

/// File information
pub struct FileInfo {
    /// File name
    pub name: String,
    /// Size in bytes
    pub size: u64,
    /// Modification time (Unix timestamp, seconds)
    pub timestamp: i64,
    /// File type
    pub ftype: FileType,
}

impl FileInfo {
    /// Whether the file is a directory.
    pub fn is_dir(&self) -> bool {
        matches!(self.ftype, FileType::DIRECTORY)
    }
}

impl<T: Write + Seek> Write for FatFile<'_, T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut bw = 0;
        let len = ff_c::UINT::try_from(buf.len()).unwrap_or(ff_c::UINT::MAX);
        if unsafe { ff_c::f_write(&mut self.inner, buf.as_ptr() as *const c_void, len, &mut bw) }
            != ff_c::FRESULT_FR_OK
        {
            Err(io::Error::other("ff write error"))
        } else {
            Ok(bw as usize)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if unsafe { ff_c::f_sync(&mut self.inner) } != ff_c::FRESULT_FR_OK {
            Err(io::Error::other("ff sync error"))
        } else {
            Ok(())
        }
    }
}

impl<T: Write + Seek> Seek for FatFile<'_, T> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let offset = match pos {
            SeekFrom::Start(start) => Some(start),
            SeekFrom::End(end) => self.inner.obj.objsize.checked_add_signed(end),
            SeekFrom::Current(current) => self.inner.fptr.checked_add_signed(current),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ff invalid seek"))?;
        if unsafe { ff_c::f_lseek(&mut self.inner, offset) } != ff_c::FRESULT_FR_OK {
            Err(io::Error::other("ff seek error"))
        } else {
            Ok(self.inner.fptr)
        }
    }
}

/// Close file before droping it
impl<T> Drop for FatFile<'_, T> {
    fn drop(&mut self) {
        if unsafe { ff_c::f_close(&mut self.inner) } != ff_c::FRESULT_FR_OK {
            eprintln!("couldn't drop ff FIL");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, sync::Mutex};

    // ff has a single global volume: serialize tests
    static LOCK: Mutex<()> = Mutex::new(());

    const SS: u32 = 512;
    const COUNT: u32 = 16384; // 8 MiB

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn new_disk() -> Cursor<Vec<u8>> {
        Cursor::new(vec![0; (SS * COUNT) as usize])
    }

    fn make_image(content: &[u8]) -> Cursor<Vec<u8>> {
        let mut fs = FatFs::mkfs(new_disk(), SS, COUNT, FM_EXFAT as u8).unwrap();
        fs.new_dir("/dir").unwrap();
        let mut file = fs.new_file("/dir/file.txt").unwrap();
        file.write_all(content).unwrap();
        drop(file);
        fs.set_timestamp("/dir/file.txt", 1_700_000_000).unwrap();
        fs.into_inner().unwrap()
    }

    #[test]
    fn roundtrip() {
        let _l = lock();
        let content = b"lorem ipsum blah blah";
        let mut disk = make_image(content);
        disk.set_position(0);

        let fs = FatFs::new(disk, SS).unwrap();
        let entries = fs.read_dir("/dir").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "file.txt");
        assert_eq!(entries[0].size, content.len() as u64);
        assert!(!entries[0].is_dir());
        assert!(fs.get_attr("/dir").unwrap().is_dir());
        assert_eq!(
            fs.get_attr("/dir/file.txt").unwrap().timestamp,
            1_700_000_000
        );

        let mut buf = vec![0; content.len()];
        let n = fs
            .read_file("/dir/file.txt", &mut buf, 0, content.len() as u64)
            .unwrap();
        assert_eq!(&buf[..n as usize], content);

        let mut buf = vec![0; 5];
        let n = fs.read_file("/dir/file.txt", &mut buf, 6, 5).unwrap();
        assert_eq!(&buf[..n as usize], b"ipsum");

        assert!(fs.read_file("/nope", &mut buf, 0, 1).is_err());
        let disk = fs.into_inner().unwrap();
        assert_eq!(disk.get_ref().len(), (SS * COUNT) as usize);
    }

    #[test]
    fn read_file_bounded_by_buf() {
        let _l = lock();
        let content = vec![0x42; 10_000];
        let fs = FatFs::new(make_image(&content), SS).unwrap();
        // bytes_to_read larger than buf must not overflow it
        let mut buf = vec![0; 100];
        let n = fs
            .read_file("/dir/file.txt", &mut buf, 0, u64::MAX)
            .unwrap();
        assert_eq!(n, 100);
        assert!(buf.iter().all(|&b| b == 0x42));
    }

    #[test]
    fn single_mount() {
        let _l = lock();
        let fs = FatFs::mkfs(new_disk(), SS, COUNT, FM_EXFAT as u8).unwrap();
        let err = FatFs::mkfs(new_disk(), SS, COUNT, FM_EXFAT as u8)
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::ResourceBusy);
        // first one still usable
        assert!(fs.read_dir("/").unwrap().is_empty());
        // dropping without into_inner() releases the slot
        drop(fs);
        let fs = FatFs::mkfs(new_disk(), SS, COUNT, FM_EXFAT as u8).unwrap();
        fs.into_inner().unwrap();
    }

    #[test]
    fn failed_mount_releases() {
        let _l = lock();
        // not a file system
        assert!(FatFs::new(new_disk(), SS).is_err());
        // too small to be read
        assert!(FatFs::new(Cursor::new(vec![0u8; 10]), SS).is_err());
        assert!(FatFs::new(new_disk(), 0).is_err());
        assert!(FatFs::new(new_disk(), 1000).is_err());
        assert!(FatFs::mkfs(new_disk(), 8192, COUNT, FM_EXFAT as u8).is_err());
        // global state is clean
        let fs = FatFs::new(make_image(b"x"), SS).unwrap();
        assert_eq!(fs.read_dir("/").unwrap().len(), 1);
    }

    #[test]
    fn non_static_inner() {
        let _l = lock();
        let mut backing = vec![0u8; (SS * COUNT) as usize];
        let fs = FatFs::mkfs(Cursor::new(&mut backing[..]), SS, COUNT, FM_EXFAT as u8).unwrap();
        fs.into_inner().unwrap();
        let fs = FatFs::new(Cursor::new(&backing[..]), SS).unwrap();
        assert!(fs.read_dir("/").unwrap().is_empty());
    }

    fn fat_roundtrip(timestamp: i64) -> i64 {
        let mut fno = ff_c::FILINFO::new();
        (fno.fdate, fno.ftime) = timestamp_to_fat(timestamp);
        fno_to_timestamp(&fno)
    }

    #[test]
    fn timestamp_clamped() {
        // in range (FAT has a 2 seconds resolution)
        assert_eq!(fat_roundtrip(1_700_000_000), 1_700_000_000);
        assert_eq!(fat_roundtrip(1_700_000_001), 1_700_000_000);
        assert_eq!(fat_roundtrip(FAT_TIMESTAMP_MIN), FAT_TIMESTAMP_MIN);
        assert_eq!(fat_roundtrip(FAT_TIMESTAMP_MAX), FAT_TIMESTAMP_MAX);
        // before 1980 / after 2107
        for ts in [i64::MIN, -1, 0, FAT_TIMESTAMP_MIN - 1] {
            assert_eq!(fat_roundtrip(ts), FAT_TIMESTAMP_MIN, "{ts}");
        }
        for ts in [
            FAT_TIMESTAMP_MAX + 1,
            4_354_819_200,
            253_402_300_800,
            i64::MAX,
        ] {
            assert_eq!(fat_roundtrip(ts), FAT_TIMESTAMP_MAX, "{ts}");
        }
        // bounds are what they claim to be
        let min = time::OffsetDateTime::from_unix_timestamp(FAT_TIMESTAMP_MIN).unwrap();
        assert_eq!(
            (min.year(), min.ordinal(), min.time()),
            (1980, 1, time::Time::MIDNIGHT)
        );
        let max = time::OffsetDateTime::from_unix_timestamp(FAT_TIMESTAMP_MAX).unwrap();
        assert_eq!(
            (max.year(), u8::from(max.month()), max.day(), max.to_hms()),
            (2107, 12, 31, (23, 59, 58))
        );
    }

    #[test]
    fn malicious_on_disk_timestamp() {
        // fields read from an untrusted file system must never panic
        for (fdate, ftime) in [(0, 0), (0xFFFF, 0xFFFF), (0x001F, 0xBFFF), (0xFFE0, 0x07FF)] {
            let mut fno = ff_c::FILINFO::new();
            fno.fdate = fdate;
            fno.ftime = ftime;
            let ts = fno_to_timestamp(&fno);
            assert!(
                (FAT_TIMESTAMP_MIN..=FAT_TIMESTAMP_MAX + 2).contains(&ts),
                "{ts}"
            );
        }
    }

    #[test]
    fn set_timestamp_extremes() {
        let _l = lock();
        let mut fs = FatFs::mkfs(new_disk(), SS, COUNT, FM_EXFAT as u8).unwrap();
        fs.new_dir("/a").unwrap();
        fs.new_dir("/b").unwrap();
        fs.set_timestamp("/a", i64::MIN).unwrap();
        fs.set_timestamp("/b", i64::MAX).unwrap();
        let fs = FatFs::new(fs.into_inner().unwrap(), SS).unwrap();
        assert_eq!(fs.get_attr("/a").unwrap().timestamp, FAT_TIMESTAMP_MIN);
        assert_eq!(fs.get_attr("/b").unwrap().timestamp, FAT_TIMESTAMP_MAX);
    }

    #[test]
    fn nul_in_path() {
        let _l = lock();
        let mut fs = FatFs::mkfs(new_disk(), SS, COUNT, FM_EXFAT as u8).unwrap();
        fs.new_dir("/a").unwrap();
        drop(fs.new_file("/f").unwrap());
        let invalid =
            |res: io::Result<()>| res.err().unwrap().kind() == io::ErrorKind::InvalidInput;
        // without the check, these would be truncated to "/a" or "/f"
        assert!(invalid(fs.new_file("/a\0b").map(drop)));
        assert!(invalid(fs.new_dir("/a\0b")));
        assert!(invalid(fs.new_dir("/b\0")));
        assert!(invalid(fs.set_timestamp("/f\0x", 0)));
        assert!(invalid(fs.remove_file("/f\0x")));
        assert!(fs.read_dir("/").unwrap().len() == 2);
        let fs = FatFs::new(fs.into_inner().unwrap(), SS).unwrap();
        assert!(invalid(fs.get_attr("/f\0x").map(drop)));
        assert!(invalid(fs.read_dir("/a\0").map(drop)));
        assert!(invalid(fs.read_file("/f\0x", &mut [0; 1], 0, 1).map(drop)));
        // f was not removed
        assert!(fs.get_attr("/f").is_ok());
    }

    #[test]
    fn file_seek() {
        let _l = lock();
        let mut fs = FatFs::mkfs(new_disk(), SS, COUNT, FM_EXFAT as u8).unwrap();
        let mut file = fs.new_file("/f").unwrap();
        file.write_all(b"0123456789").unwrap();
        assert_eq!(file.seek(SeekFrom::End(-4)).unwrap(), 6);
        assert_eq!(file.seek(SeekFrom::Current(-2)).unwrap(), 4);
        assert_eq!(file.seek(SeekFrom::Start(1)).unwrap(), 1);
        assert!(file.seek(SeekFrom::Current(-5)).is_err());
        file.write_all(b"ab").unwrap();
        drop(file);
        let disk = fs.into_inner().unwrap();
        let fs = FatFs::new(disk, SS).unwrap();
        let mut buf = [0; 10];
        fs.read_file("/f", &mut buf, 0, 10).unwrap();
        assert_eq!(&buf, b"0ab3456789");
    }
}
