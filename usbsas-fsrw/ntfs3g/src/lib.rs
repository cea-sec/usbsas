//! ntfs3g bindings
//!
//! This crate only supports creating and writing a new NTFS file system.

use std::{
    convert::TryFrom,
    ffi::{CStr, CString},
    io::{self, Error, ErrorKind, Read, Result, Seek, SeekFrom, Write},
    marker::PhantomData,
    os::raw::{c_char, c_long, c_void},
    sync::Mutex,
};

mod n3g_c;
mod n3g_extern;

/// `Read + Write + Seek`, the stream holding the file system.
pub trait ReadWriteSeek: Read + Write + Seek {}
impl<T: Read + Write + Seek> ReadWriteSeek for T {}

/// mkntfs() works on global state (`opts`, `g_vol`, ...): serialize calls.
static MKNTFS_LOCK: Mutex<()> = Mutex::new(());

/// Name given to ntfs-3g for the device, unused since I/O are redirected.
const DEV_NAME: &CStr = c"";

fn split_path_parent(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some(("", file)) => ("/", file),
        Some((parent, file)) => (parent, file),
        None => ("/", path),
    }
}

/// Validate a file name and convert it to NTFS unicode. Names come from
/// untrusted input and ntfs_create() doesn't check them.
fn str2ntfsunicode(name: &str) -> Result<Vec<u16>> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['\0', '/']) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("ntfs3g invalid ntfs name: {name:?}"),
        ));
    }
    let string_u16: Vec<u16> = name.encode_utf16().collect();
    if string_u16.len() > n3g_c::NTFS_MAX_NAME_LEN as usize {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "ntfs3g ntfs name too long",
        ));
    }
    Ok(string_u16)
}

fn to_cstring(string: &str) -> Result<CString> {
    CString::new(string).map_err(|_| {
        Error::new(
            ErrorKind::InvalidInput,
            format!("ntfs3g invalid path: {string:?}"),
        )
    })
}

fn n3g_error(func: &str) -> Error {
    let err = Error::last_os_error();
    Error::new(err.kind(), format!("ntfs3g {func} error ({err})"))
}

/// An opened ntfs-3g inode, closed on drop.
struct Inode(*mut n3g_c::ntfs_inode);

impl Inode {
    /// Take ownership of the inode returned by an ntfs-3g function or fail
    /// with errno.
    fn from_raw(ni: *mut n3g_c::ntfs_inode, func: &str) -> Result<Self> {
        if ni.is_null() {
            Err(n3g_error(func))
        } else {
            Ok(Inode(ni))
        }
    }

    /// Give back ownership (e.g. to a function that closes it).
    fn into_raw(self) -> *mut n3g_c::ntfs_inode {
        let ni = self.0;
        std::mem::forget(self);
        ni
    }

    fn set_times(&mut self, timestamp: i64) {
        // Ntfs timestamp is a 64-bit value representing the number of
        // 100-nanosecond intervals since January 1, 1601 (UTC)
        let ntfs_ts: u64 = ((timestamp + 11644473600) * 10 * 1000 * 1000) as u64;
        unsafe {
            (*self.0).creation_time = ntfs_ts;
            (*self.0).last_data_change_time = ntfs_ts;
            (*self.0).last_mft_change_time = ntfs_ts;
            (*self.0).last_access_time = ntfs_ts;
        }
    }

    /// Close and report errors (the inode is written back on close).
    fn close(self) -> Result<()> {
        if unsafe { n3g_c::ntfs_inode_close(self.into_raw()) } != 0 {
            Err(n3g_error("ntfs_inode_close"))
        } else {
            Ok(())
        }
    }
}

impl Drop for Inode {
    fn drop(&mut self) {
        if unsafe { n3g_c::ntfs_inode_close(self.0) } != 0 {
            eprintln!("Couldn't close ntfs inode");
        }
    }
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
        {
            let _lock = MKNTFS_LOCK.lock().unwrap_or_else(|err| err.into_inner());
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
                (*opts).sector_size = c_long::try_from(sector_size).map_err(|_| {
                    Error::new(ErrorKind::InvalidInput, "ntfs3g invalid sector size")
                })?;
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
        }

        fs.vol = unsafe { n3g_c::ntfs_mount(DEV_NAME.as_ptr(), 0, fs.io) };
        if fs.vol.is_null() {
            return Err(n3g_error("ntfs_mount"));
        }
        Ok(fs)
    }

    fn inode_from_path(&self, path: &str) -> Result<Inode> {
        let path_c = to_cstring(path)?;
        Inode::from_raw(
            unsafe {
                n3g_c::ntfs_pathname_to_inode(self.vol, std::ptr::null_mut(), path_c.as_ptr())
            },
            "ntfs_pathname_to_inode",
        )
    }

    /// Create an inode of type `mode` named `path`, its parent must exist.
    fn create(&mut self, path: &str, mode: u32) -> Result<Inode> {
        let (parent_dir, filename) = split_path_parent(path);
        let name = str2ntfsunicode(filename)?;
        let p_ni = self.inode_from_path(parent_dir)?;
        let ni = unsafe {
            n3g_c::ntfs_create(
                p_ni.0,
                0,
                name.as_ptr(),
                // checked by str2ntfsunicode: <= 255
                name.len() as u8,
                mode,
            )
        };
        // build the result (capturing errno) before closing the parent
        let ni = Inode::from_raw(ni, "ntfs_create");
        drop(p_ni);
        ni
    }

    /// Create a new file and returns an NtfsAttr (that impl Write and Seek)
    pub fn new_file<'a>(&'a mut self, path: &str, timestamp: i64) -> Result<NtfsAttr<'a, T>> {
        let mut file_ni = self.create(path, n3g_c::S_IFREG)?;
        file_ni.set_times(timestamp);

        let file_na = unsafe {
            n3g_c::ntfs_attr_open(
                file_ni.0,
                n3g_c::ATTR_TYPES_AT_DATA,
                std::ptr::null_mut(),
                0,
            )
        };
        if file_na.is_null() {
            // file_ni is closed on drop
            return Err(n3g_error("ntfs_attr_open"));
        }

        // closed by NtfsAttr's drop, through (*attr).ni
        file_ni.into_raw();
        Ok(NtfsAttr {
            attr: file_na,
            pos: 0,
            _fs: PhantomData,
        })
    }

    /// Create a directory, its parent must exist.
    pub fn new_dir(&mut self, path: &str, timestamp: i64) -> Result<()> {
        let mut dir_ni = match self.create(path, n3g_c::S_IFDIR) {
            Ok(dir_ni) => dir_ni,
            Err(err) if err.kind() == ErrorKind::AlreadyExists => return Ok(()),
            Err(err) => return Err(err),
        };
        dir_ni.set_times(timestamp);
        dir_ni.close()
    }

    /// Remove a file.
    pub fn remove_file(&mut self, path: &str) -> Result<()> {
        let (parent_dir, filename) = split_path_parent(path);
        let name = str2ntfsunicode(filename)?;
        let name_c = to_cstring(filename)?;
        let p_ni = self.inode_from_path(parent_dir)?;
        let ni = self.inode_from_path(path)?;

        if unsafe {
            n3g_c::ntfs_delete(
                self.vol,
                name_c.as_ptr(),
                ni.into_raw(),
                p_ni.into_raw(),
                name.as_ptr(),
                // checked by str2ntfsunicode: <= 255
                name.len() as u8,
            )
        } != 0
        {
            return Err(n3g_error("ntfs_delete"));
        }
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
    fn invalid_names() {
        let mut fs = Ntfs3g::mkntfs(new_disk(), SS, COUNT).unwrap();
        let long = "a".repeat(256);
        for path in [
            "/",
            "/dir/",
            "/.",
            "/..",
            "/a\0b",
            "/a\0b/c",
            &format!("/{long}"),
        ] {
            assert_eq!(
                fs.new_file(path, 0).err().unwrap().kind(),
                ErrorKind::InvalidInput,
                "{path:?}"
            );
            assert!(fs.new_dir(path, 0).is_err(), "{path:?}");
            assert!(fs.remove_file(path).is_err(), "{path:?}");
        }
        fs.new_file(&format!("/{}", "a".repeat(255)), 0).unwrap();
        // relative paths are relative to the root
        fs.new_file("rel", 0).unwrap();
        fs.remove_file("/rel").unwrap();
    }

    #[test]
    fn drop_without_into_inner() {
        let mut fs = Ntfs3g::mkntfs(new_disk(), SS, COUNT).unwrap();
        fs.new_dir("/dir", 0).unwrap();
        drop(fs);
    }

    #[test]
    fn concurrent_mkntfs() {
        let threads: Vec<_> = (0..4)
            .map(|i| {
                std::thread::spawn(move || {
                    let mut fs = Ntfs3g::mkntfs(new_disk(), SS, COUNT).unwrap();
                    fs.new_dir(&format!("/dir{i}"), 0).unwrap();
                    fs.into_inner().unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
    }
}
