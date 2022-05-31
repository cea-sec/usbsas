//! ff bindings
//!
//! This crate can be used to read an already existing FAT file system or to
//! write a new one. Modifying an already existing file system is (not yet ?)
//! supported.
//!
//! The struct `FatFs<T>` is a wrapper around ff's `FATFS` struct. It implements
//! reading a file system when `T` is `Read` + `Seek`, and writing a file system
//! when `T` is `Read` + `Write` + `Seek`.

use std::{
    io::{self, ErrorKind, Read, Seek, SeekFrom, Write},
    marker::PhantomData,
    os::raw::c_void,
};

mod ff_c;
mod ff_extern;
mod wrapper;
use wrapper::{WrapperFatFs, WrapperRead, WrapperReadWrite};

pub use ff_c::{FM_EXFAT, FM_FAT32};

const DRIVE: &str = "0:";

fn str_to_utf16(string: &str) -> Vec<u16> {
    let mut ret: Vec<u16> = string.encode_utf16().collect();
    ret.push(0);
    ret
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

/// Wrapper around ff's FATFS struct
pub struct FatFs<T> {
    // Box here because ff keeps a pointer to the FATFS struct so it mustn't move
    inner: Box<ff_c::FATFS>,
    inner_type: PhantomData<T>,
}

impl<T: Read + Seek> FatFs<T> {
    /// Parse a FAT filesystem from T
    pub fn new(inner: T, sector_size: u32) -> Result<Self, io::Error> {
        // Here we allocate a new FATFS struct and mount it.
        // We use the void* pdrv field of the struct in order to get the inner reader in the extern
        // C functions (ff_extern.rs). In those functions we don't know what T is so we must have
        // a Box<dyn Trait>.
        // We wrap this box into a second one for 2 reasons:
        //  - We can use Box::into_raw() / Box::from_raw() to cast into / from void*. We can't do
        //    that with a simple box because dyn Trait in a fat pointer.
        //  - The first box will be on the heap and won't move. We could have Pinned it but it would
        //    require T to impl Unpin for the into_inner() function.
        let inner_box: Box<Box<dyn WrapperFatFs>> =
            Box::new(Box::new(WrapperRead::new(inner, sector_size)));
        let mut fs = Box::new(ff_c::FATFS::new());
        fs.pdrv = Box::into_raw(inner_box) as *mut c_void;
        let drive = str_to_utf16(DRIVE);

        if unsafe { ff_c::f_mount(&mut *fs, drive.as_ptr(), 1) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::new(ErrorKind::Other, "ff f_mount error"));
        }
        Ok(FatFs {
            inner: fs,
            inner_type: PhantomData,
        })
    }

    /// Unmount fs and return T
    pub fn into_inner_r(self) -> Result<T, io::Error> {
        let inner: Box<Box<WrapperRead<T>>> =
            unsafe { Box::from_raw(self.inner.pdrv as *mut Box<WrapperRead<T>>) };
        let drive = str_to_utf16(DRIVE);
        if unsafe { ff_c::f_mount(std::ptr::null_mut(), drive.as_ptr(), 0) } != ff_c::FRESULT_FR_OK
        {
            eprintln!("f_mount (unmount) error");
        }
        Ok((*(*inner)).into_inner())
    }

    /// Get file attributes from path
    pub fn get_attr(&self, path: &str) -> Result<FileInfo, io::Error> {
        let path_u16 = str_to_utf16(path);
        let mut fno = ff_c::FILINFO::new();
        if unsafe { ff_c::f_stat(path_u16.as_ptr(), &mut fno) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::new(ErrorKind::Other, "ff f_atrib error"));
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
        let mut files_info = Vec::new();
        let path_u16 = str_to_utf16(path);

        if unsafe { ff_c::f_opendir(&mut dir, path_u16.as_ptr()) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::new(ErrorKind::Other, "ff f_opendir error"));
        }

        loop {
            let mut fno = ff_c::FILINFO::new();

            if unsafe { ff_c::f_readdir(&mut dir, &mut fno) } != ff_c::FRESULT_FR_OK {
                return Err(io::Error::new(ErrorKind::Other, "ff f_readir error"));
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
                Err(_) => return Err(io::Error::new(ErrorKind::Other, "ff from_utf16 error")),
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

        unsafe { ff_c::f_closedir(&mut dir) };

        Ok(files_info)
    }

    /// Read `bytes_to_read` at `offset` from file in `path `into `buf`
    pub fn read_file(
        &self,
        path: &str,
        buf: &mut [u8],
        offset: u64,
        bytes_to_read: u64,
    ) -> Result<u64, io::Error> {
        let mut fp = ff_c::FIL::new();
        let mut bytes_read: ff_c::UINT = 0;
        let path_u16 = str_to_utf16(path);

        if unsafe { ff_c::f_open(&mut fp, path_u16.as_ptr(), ff_c::FA_READ as u8) }
            != ff_c::FRESULT_FR_OK
        {
            return Err(io::Error::new(ErrorKind::Other, "ff f_open error"));
        }
        if unsafe { ff_c::f_lseek(&mut fp, offset) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::new(ErrorKind::Other, "ff f_lseek error"));
        }
        if unsafe {
            ff_c::f_read(
                &mut fp,
                buf.as_mut_ptr() as *mut std::os::raw::c_void,
                bytes_to_read as u32,
                &mut bytes_read,
            )
        } != ff_c::FRESULT_FR_OK
        {
            Err(io::Error::new(ErrorKind::Other, "ff f_read error"))
        } else {
            Ok(bytes_read as u64)
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
        // See fn new() above about the double Box.
        let inner_box: Box<Box<dyn WrapperFatFs>> =
            Box::new(Box::new(WrapperReadWrite::new(inner, sector_size)));
        let mut fs = Box::new(ff_c::FATFS::new());
        fs.pdrv = Box::into_raw(inner_box) as *mut c_void;
        let drive = str_to_utf16(DRIVE);

        let mkfs_params = ff_c::MKFS_PARM {
            fmt: fstype,
            n_fat: 0,
            align: 4096 / sector_size,
            n_root: 0,
            au_size: 0,
            sz_vol: sector_count,
        };
        if unsafe {
            ff_c::f_mkfs(
                fs.pdrv,
                drive.as_ptr(),
                &mkfs_params,
                std::ptr::null_mut(),
                4096,
            )
        } != ff_c::FRESULT_FR_OK
        {
            return Err(io::Error::new(ErrorKind::Other, "ff f_mkfs error"));
        }

        if unsafe { ff_c::f_mount(&mut *fs, drive.as_ptr(), 1) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::new(ErrorKind::Other, "ff f_mount error"));
        }

        Ok(FatFs {
            inner: fs,
            inner_type: PhantomData,
        })
    }

    /// Unmount fs and return T
    pub fn into_inner_rw(self) -> Result<T, io::Error> {
        let mut inner: Box<Box<WrapperReadWrite<T>>> =
            unsafe { Box::from_raw(self.inner.pdrv as *mut Box<WrapperReadWrite<T>>) };
        (*(*inner)).flush()?;
        let drive = str_to_utf16(DRIVE);
        if unsafe { ff_c::f_mount(std::ptr::null_mut(), drive.as_ptr(), 0) } != ff_c::FRESULT_FR_OK
        {
            eprintln!("f_mount (unmount) error");
        }
        Ok((*(*inner)).into_inner())
    }

    /// Create a new file on `path`. Returns a `Write`able `FatFile`.
    pub fn new_file<'a>(&'a mut self, path: &str) -> Result<FatFile<'a, T>, io::Error> {
        let fname = str_to_utf16(path);
        let mut file = FatFile::new(ff_c::FIL::new());
        if unsafe {
            ff_c::f_open(
                &mut file.inner,
                fname.as_ptr(),
                (ff_c::FA_WRITE | ff_c::FA_CREATE_NEW) as u8,
            )
        } != ff_c::FRESULT_FR_OK
        {
            return Err(io::Error::new(ErrorKind::Other, "ff f_open error"));
        }
        Ok(file)
    }

    /// Create a new directory on `path`
    pub fn new_dir(&mut self, path: &str) -> Result<(), io::Error> {
        let dname = str_to_utf16(path);
        match unsafe { ff_c::f_mkdir(dname.as_ptr()) } {
            ff_c::FRESULT_FR_OK | ff_c::FRESULT_FR_EXIST => Ok(()),
            _ => Err(io::Error::new(ErrorKind::Other, "ff f_mkdir error")),
        }
    }

    /// Remove file on `path`
    pub fn remove_file(&mut self, path: &str) -> Result<(), io::Error> {
        let fname = str_to_utf16(path);
        if unsafe { ff_c::f_unlink(fname.as_ptr()) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::new(ErrorKind::Other, "ff f_mkdir error"));
        }
        Ok(())
    }

    /// Set UNIX timestamp on file `path`
    pub fn set_timestamp(&mut self, path: &str, timestamp: i64) -> Result<(), io::Error> {
        let fname = str_to_utf16(path);
        let dt = time::OffsetDateTime::from_unix_timestamp(timestamp).unwrap_or_else(|_| {
            time::OffsetDateTime::from_unix_timestamp(315532800) // 01.01.1980 00:00:00
                .unwrap()
        });
        let mut fno = ff_c::FILINFO::new();
        fno.fdate = ((dt.year() as u16 - 1980) << 9)
            | ((u8::from(dt.month()) as u16) << 5)
            | (dt.day() as u16);
        fno.ftime =
            ((dt.hour() as u16) << 11) | ((dt.minute() as u16) << 5) | (dt.second() as u16 / 2);
        if unsafe { ff_c::f_utime(fname.as_ptr(), &fno) } != ff_c::FRESULT_FR_OK {
            return Err(io::Error::new(ErrorKind::Other, "ff f_utime error"));
        }
        Ok(())
    }
}

/// Wrapper around ff's `FIL` struct. The lifetime is bound to the underlying
/// file system `FatFs<T>`.
pub struct FatFile<'a, T> {
    pub(crate) inner: ff_c::FIL,
    // Lifetimed ref to underlying FatFs so we don't outlive it
    _fsref: PhantomData<&'a FatFs<T>>,
}

impl<'a, T> FatFile<'a, T> {
    pub fn new(inner: ff_c::FIL) -> Self {
        FatFile {
            inner,
            _fsref: PhantomData,
        }
    }
}

pub enum FileType {
    REGULAR,
    DIRECTORY,
}

/// File information
pub struct FileInfo {
    pub name: String,
    pub size: u64,
    pub timestamp: i64,
    pub ftype: FileType,
}

impl FileInfo {
    pub fn is_dir(&self) -> bool {
        matches!(self.ftype, FileType::DIRECTORY)
    }
}

impl<T: Write + Seek> Write for FatFile<'_, T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut bw = 0;
        if unsafe {
            ff_c::f_write(
                &mut self.inner,
                buf.as_ptr() as *mut ::std::os::raw::c_void,
                buf.len() as ff_c::UINT,
                &mut bw,
            )
        } != ff_c::FRESULT_FR_OK
        {
            Err(io::Error::new(io::ErrorKind::Other, "ff write error"))
        } else {
            Ok(bw as usize)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if unsafe { ff_c::f_sync(&mut self.inner) } != ff_c::FRESULT_FR_OK {
            Err(io::Error::new(io::ErrorKind::Other, "ff sync error"))
        } else {
            Ok(())
        }
    }
}

impl<T: Write + Seek> Seek for FatFile<'_, T> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let offset = match pos {
            SeekFrom::Start(start) => start,
            SeekFrom::End(end) => self.inner.fptr - end as u64,
            SeekFrom::Current(current) => self.inner.fptr + current as u64,
        };
        if unsafe { ff_c::f_lseek(&mut self.inner, offset) } != ff_c::FRESULT_FR_OK {
            Err(io::Error::new(io::ErrorKind::Other, "ff seek error"))
        } else {
            Ok(offset)
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
