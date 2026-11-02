//! ntfs3g bindings
//!
//! This crate only supports creating and writing a new NTFS file system.

use std::{
    convert::TryFrom,
    ffi::{CStr, CString},
    io::{self, Error, ErrorKind, Read, Result, Seek, SeekFrom, Write},
    marker::PhantomData,
    os::raw::{c_char, c_long, c_void},
};

mod n3g_c;
mod n3g_extern;

/// `Read + Write + Seek`, the stream holding the file system.
pub trait ReadWriteSeek: Read + Write + Seek {}
impl<T: Read + Write + Seek> ReadWriteSeek for T {}

/// Name given to ntfs-3g for the device, unused since I/O are redirected.
const DEV_NAME: &CStr = c"";

fn split_path_parent(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some((parent, file)) => (parent, file),
        None => ("/", path),
    }
}

fn str2ntfsunicode(string: &str) -> Result<Vec<u16>> {
    let string_u16: Vec<u16> = string.encode_utf16().collect();
    if string_u16.len() > n3g_c::NTFS_MAX_NAME_LEN as usize {
        return Err(Error::other("ntfs3g invalid ntfs name"));
    }
    Ok(string_u16)
}

fn n3g_error(func: &str) -> Error {
    let err = Error::last_os_error();
    Error::new(err.kind(), format!("ntfs3g {func} error ({err})"))
}

/// New NTFS file system, created with [`Ntfs3g::mkntfs`].
pub struct Ntfs3g<T> {
    // Both pointers are owned by this struct and released in release().
    // `vol` is null until mounted. `io` is the device's d_private: a thin
    // pointer to a `Box<dyn ReadWriteSeek + '_>` (fat, hence the double box)
    // whose concrete type is `T`. ntfs-3g keeps aliases to it and the
    // callbacks mutate it, so it's kept as a raw pointer rather than a Box
    // (which would assert unique ownership). Null once released.
    vol: *mut n3g_c::ntfs_volume,
    io: *mut c_void,
    _inner: PhantomData<T>,
}

impl<T> Ntfs3g<T> {
    /// Unmount, free ntfs-3g's state and give back T along with the result of
    /// unmounting and flushing. Returns None if already released.
    fn release(&mut self) -> Option<(T, Result<()>)> {
        let io = std::mem::replace(&mut self.io, std::ptr::null_mut());
        if io.is_null() {
            return None;
        }
        let vol = std::mem::replace(&mut self.vol, std::ptr::null_mut());
        // ntfs_umount() writes back dirty data through the callbacks, so it
        // must run before `io` is reclaimed. It frees `vol` and its device
        // even when it fails.
        let mut res = if !vol.is_null() && unsafe { n3g_c::ntfs_umount(vol, n3g_c::BOOL_TRUE) } != 0
        {
            Err(n3g_error("ntfs_umount"))
        } else {
            Ok(())
        };
        // SAFETY: `io` comes from Box::into_raw in mkntfs(), ntfs-3g doesn't
        // reference it anymore and it's released only once (io is now null).
        // The `dyn ReadWriteSeek` was created from a `Box<T>` so the thin
        // pointer cast is to the real type and from_raw deallocates with the
        // original layout.
        let inner = unsafe {
            let outer: Box<Box<dyn ReadWriteSeek + '_>> =
                Box::from_raw(io as *mut Box<dyn ReadWriteSeek>);
            let mut stream: Box<dyn ReadWriteSeek + '_> = *outer;
            if let Err(err) = stream.flush() {
                res = res.and(Err(err));
            }
            *Box::from_raw(Box::into_raw(stream) as *mut T)
        };
        Some((inner, res))
    }

    /// Unmount the file system and return the underlying stream.
    pub fn into_inner(mut self) -> Result<T> {
        let (inner, res) = self
            .release()
            .ok_or_else(|| Error::other("ntfs3g: already released"))?;
        res.map(|()| inner)
    }
}

/// Unmount and free everything if `into_inner()` wasn't called
impl<T> Drop for Ntfs3g<T> {
    fn drop(&mut self) {
        if let Some((_, Err(err))) = self.release() {
            eprintln!("ntfs3g unmount on drop failed: {err}");
        }
    }
}

impl<T: Read + Write + Seek> Ntfs3g<T> {
    /// Create an NTFS file system on `inner` (`sector_count` sectors of
    /// `sector_size` bytes) and mount it.
    pub fn mkntfs(inner: T, sector_size: i64, sector_count: i64) -> Result<Self> {
        let stream: Box<dyn ReadWriteSeek + '_> = Box::new(inner);
        // From here on, returning an error drops `fs` which frees everything
        let mut fs = Ntfs3g {
            vol: std::ptr::null_mut(),
            io: Box::into_raw(Box::new(stream)) as *mut c_void,
            _inner: PhantomData,
        };

        unsafe {
            // mkntfs() reads its options from the global `opts`
            // (`opts2` is only checked for NULL).
            // See original mkntfs.c from ntfsprogs
            let opts = &raw mut n3g_c::opts;
            std::ptr::write_bytes(opts, 0, 1);
            (*opts).cluster_size = -1;
            (*opts).mft_zone_multiplier = -1;
            (*opts).with_uuid = n3g_c::BOOL_FALSE;
            (*opts).use_epoch_time = n3g_c::BOOL_TRUE;
            (*opts).sector_size = c_long::try_from(sector_size)
                .map_err(|_| Error::new(ErrorKind::InvalidInput, "ntfs3g invalid sector size"))?;
            (*opts).num_sectors = sector_count;
            (*opts).part_start_sect = 0;
            (*opts).heads = 1;
            (*opts).sectors_per_track = 63;
            (*opts).quick_format = n3g_c::BOOL_TRUE;
            (*opts).dev_name = DEV_NAME.as_ptr() as *mut c_char;
            let ret = n3g_c::mkntfs(opts, fs.io);
            std::ptr::write_bytes(opts, 0, 1);
            if ret != 0 {
                return Err(Error::other("ntfs3g mkntfs error"));
            }
        }

        fs.vol = unsafe { n3g_c::ntfs_mount(DEV_NAME.as_ptr(), 0, fs.io) };
        if fs.vol.is_null() {
            return Err(n3g_error("ntfs_mount"));
        }
        Ok(fs)
    }

    fn inode_from_path(&self, path: &str) -> Result<*mut n3g_c::ntfs_inode> {
        let path_c = CString::new(path).unwrap();
        let ni: *mut n3g_c::ntfs_inode = unsafe {
            n3g_c::ntfs_pathname_to_inode(&mut *self.vol, std::ptr::null_mut(), path_c.as_ptr())
        };
        if ni.is_null() {
            Err(Error::other(format!(
                "ntfs3g ntfs_pathname_to_inode error ({})",
                Error::last_os_error()
            )))
        } else {
            Ok(ni)
        }
    }

    /// Create a new file and returns an NtfsAttr (that impl Write and Seek)
    pub fn new_file<'a>(&'a mut self, path: &str, timestamp: i64) -> Result<NtfsAttr<'a, T>> {
        let (parent_dir, filename) = split_path_parent(path);
        let p_ni = self.inode_from_path(parent_dir)?;
        let path_u16 = str2ntfsunicode(filename)?;

        let file_ni = unsafe {
            n3g_c::ntfs_create(
                p_ni,
                0,
                path_u16.as_ptr(),
                path_u16.len() as u8,
                n3g_c::S_IFREG,
            )
        };
        unsafe { n3g_c::ntfs_inode_close(p_ni) };
        if file_ni.is_null() {
            return Err(Error::other(format!(
                "ntfs3g ntfs_create error ({})",
                Error::last_os_error()
            )));
        }

        let file_na = unsafe {
            n3g_c::ntfs_attr_open(file_ni, n3g_c::ATTR_TYPES_AT_DATA, std::ptr::null_mut(), 0)
        };
        if file_na.is_null() {
            return Err(Error::other(format!(
                "ntfs3g ntfs_attr_open error ({})",
                Error::last_os_error()
            )));
        }

        // Set timestamp.
        // Ntfs timestamp is a 64-bit value representing the number of 100-nanosecond intervals
        // since January 1, 1601 (UTC)
        let ntfs_ts: u64 = ((timestamp + 11644473600) * 10 * 1000 * 1000) as u64;
        unsafe {
            (*file_ni).creation_time = ntfs_ts;
            (*file_ni).last_data_change_time = ntfs_ts;
            (*file_ni).last_mft_change_time = ntfs_ts;
            (*file_ni).last_access_time = ntfs_ts;
        }

        Ok(NtfsAttr {
            attr: file_na,
            pos: 0,
            _fs: PhantomData,
        })
    }

    /// Create a directory, its parent must exist.
    pub fn new_dir(&mut self, path: &str, timestamp: i64) -> Result<()> {
        let (parent_dir, filename) = split_path_parent(path);
        let p_ni = self.inode_from_path(parent_dir)?;
        let path_u16 = str2ntfsunicode(filename)?;

        let dir_ni = unsafe {
            n3g_c::ntfs_create(
                p_ni,
                0,
                path_u16.as_ptr(),
                path_u16.len() as u8,
                n3g_c::S_IFDIR,
            )
        };
        unsafe { n3g_c::ntfs_inode_close(p_ni) };
        if dir_ni.is_null() {
            if let ErrorKind::AlreadyExists = Error::last_os_error().kind() {
                return Ok(());
            } else {
                return Err(Error::other(format!(
                    "ntfs3g ntfs_create error ({})",
                    Error::last_os_error()
                )));
            }
        }

        // Set timestamp
        let ntfs_ts: u64 = ((timestamp + 11644473600) * 10 * 1000 * 1000) as u64;
        unsafe {
            (*dir_ni).creation_time = ntfs_ts;
            (*dir_ni).last_data_change_time = ntfs_ts;
            (*dir_ni).last_mft_change_time = ntfs_ts;
            (*dir_ni).last_access_time = ntfs_ts;
        }

        unsafe { n3g_c::ntfs_inode_close(dir_ni) };

        Ok(())
    }

    /// Remove a file.
    pub fn remove_file(&mut self, path: &str) -> Result<()> {
        let (parent_dir, filename) = split_path_parent(path);
        let p_ni = self.inode_from_path(parent_dir)?;
        let ni = self.inode_from_path(path)?;
        let path_u16 = str2ntfsunicode(filename)?;
        let path_c = CString::new(filename)
            .map_err(|err| Error::other(format!("ntfs cstring error ({err})")))?;

        if unsafe {
            n3g_c::ntfs_delete(
                self.vol,
                path_c.as_ptr(),
                ni,
                p_ni,
                path_u16.as_ptr(),
                path_u16.len() as u8,
            )
        } != 0
        {
            return Err(Error::other(format!(
                "ntfs3g ntfs_delete error ({})",
                Error::last_os_error()
            )));
        }
        unsafe { n3g_c::ntfs_inode_close(p_ni) };
        Ok(())
    }
}

/// Data attribute of a file being written, closed when dropped.
pub struct NtfsAttr<'a, T: Read + Write + Seek> {
    /// ntfs-3g attribute
    pub attr: *mut n3g_c::ntfs_attr,
    /// Current position
    pub pos: u64,
    _fs: PhantomData<&'a Ntfs3g<T>>,
}

impl<T: Read + Write + Seek> Write for NtfsAttr<'_, T> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let ret = unsafe {
            n3g_c::ntfs_attr_pwrite(
                self.attr,
                i64::try_from(self.pos)
                    .map_err(|_| Error::new(ErrorKind::InvalidData, "tryfrom err"))?,
                i64::try_from(buf.len())
                    .map_err(|_| Error::new(ErrorKind::InvalidData, "tryfrom err"))?,
                buf.as_ptr() as *mut ::std::os::raw::c_void,
            )
        };
        if ret > 0 {
            self.pos += ret as u64;
            Ok(ret as usize)
        } else {
            Err(io::Error::other("NtfsAttr Write error"))
        }
    }

    fn flush(&mut self) -> Result<()> {
        if unsafe { n3g_c::ntfs_inode_sync((*self.attr).ni) } == 0 {
            Ok(())
        } else {
            Err(Error::other("ntfs inode sync error"))
        }
    }
}

impl<T: Read + Write + Seek> Seek for NtfsAttr<'_, T> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let data_size = unsafe { (*self.attr).data_size };
        match pos {
            SeekFrom::Start(start) => {
                if start <= data_size as u64 {
                    self.pos = start
                } else {
                    return Err(Error::other("ntfs inode seek error from start"));
                }
            }
            SeekFrom::End(end) => {
                if end <= data_size {
                    self.pos = (data_size - end) as u64;
                } else {
                    return Err(Error::other("ntfs inode seek error from end"));
                }
            }
            SeekFrom::Current(current) => {
                if i64::try_from(self.pos)
                    .map_err(|_| Error::new(ErrorKind::InvalidData, "tryfrom err"))?
                    + current
                    <= data_size
                {
                    self.pos = (i64::try_from(self.pos)
                        .map_err(|_| Error::new(ErrorKind::InvalidData, "tryfrom err"))?
                        + current) as u64
                } else {
                    return Err(Error::other("ntfs inode seek error from cur"));
                }
            }
        }
        Ok(self.pos)
    }
}

impl<T: Read + Write + Seek> Drop for NtfsAttr<'_, T> {
    fn drop(&mut self) {
        self.flush().ok();
        unsafe {
            let ni = (*self.attr).ni;
            n3g_c::ntfs_attr_pclose(self.attr);
            n3g_c::ntfs_attr_close(self.attr);
            if n3g_c::ntfs_inode_close(ni) != 0 {
                eprintln!("Couldn't close ntfs inode");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const SS: i64 = 512;
    const COUNT: i64 = 32768; // 16 MiB

    fn new_disk() -> Cursor<Vec<u8>> {
        Cursor::new(vec![0; (SS * COUNT) as usize])
    }

    #[test]
    fn drop_without_into_inner() {
        let mut fs = Ntfs3g::mkntfs(new_disk(), SS, COUNT).unwrap();
        fs.new_dir("/dir", 0).unwrap();
        drop(fs);
    }
}
