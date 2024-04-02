//! ntfs3g bindings
//!
//! This crate only supports creating and writing a new NTFS file system.

use std::{
    convert::TryFrom,
    ffi::CString,
    io::{self, Error, ErrorKind, Read, Result, Seek, SeekFrom, Write},
    marker::PhantomData,
    os::raw::c_void,
};

mod n3g_c;
mod n3g_extern;

pub trait ReadWriteSeek: Read + Write + Seek {}
impl<T: Read + Write + Seek> ReadWriteSeek for T {}

pub struct Ntfs3g<T> {
    vol: *mut n3g_c::ntfs_volume,
    inner_type: PhantomData<T>,
}

fn split_path_parent(path: &str) -> Result<(String, String)> {
    let (mut parent_dir, filename) = match path.rsplit_once('/') {
        Some((parent, filename)) => (parent.to_owned(), filename.to_owned()),
        None => return Err(Error::new(ErrorKind::Other, "ntfs3g bad filename")),
    };
    if parent_dir.is_empty() {
        parent_dir = String::from("/");
    }
    Ok((parent_dir, filename))
}

fn str2ntfsunicode(string: &str) -> Result<Vec<u16>> {
    let string_u16: Vec<u16> = string.encode_utf16().collect();
    if string_u16.len() > n3g_c::NTFS_MAX_NAME_LEN as usize {
        return Err(Error::new(ErrorKind::Other, "ntfs3g invalid ntfs name"));
    }
    Ok(string_u16)
}

impl<T: Read + Write + Seek> Ntfs3g<T> {
    pub fn mkntfs(inner: T, sector_size: i64, sector_count: i64) -> Result<Self> {
        let inner_box: Box<Box<dyn ReadWriteSeek>> = Box::new(Box::new(inner));
        let priv_data = Box::into_raw(inner_box) as *mut c_void;

        let dev_name = CString::new("").unwrap();
        unsafe {
            // See original mkntfs.c from ntfsprogs
            std::ptr::write_bytes::<n3g_c::mkntfs_options>(
                std::ptr::addr_of_mut!(n3g_c::opts),
                0,
                1,
            );
            n3g_c::opts.cluster_size = -1;
            n3g_c::opts.mft_zone_multiplier = -1;
            n3g_c::opts.with_uuid = n3g_c::BOOL_FALSE;
            n3g_c::opts.use_epoch_time = n3g_c::BOOL_TRUE;
            #[cfg(not(target_arch = "arm"))]
            {
                n3g_c::opts.sector_size = sector_size;
            }
            #[cfg(target_arch = "arm")]
            {
                n3g_c::opts.sector_size = i32::try_from(sector_size)
                    .map_err(|_| Error::new(ErrorKind::InvalidData, "tryfrom err"))?;
            }
            n3g_c::opts.num_sectors = sector_count;
            n3g_c::opts.part_start_sect = 0;
            n3g_c::opts.heads = 1;
            n3g_c::opts.sectors_per_track = 63;
            n3g_c::opts.quick_format = n3g_c::BOOL_TRUE;
            #[cfg(not(any(target_arch = "arm", target_arch = "aarch64")))]
            {
                n3g_c::opts.dev_name = dev_name.as_ptr() as *mut i8;
            }
            #[cfg(any(target_arch = "arm", target_arch = "aarch64"))]
            {
                n3g_c::opts.dev_name = dev_name.as_ptr() as *mut u8;
            }
            if n3g_c::mkntfs(std::ptr::addr_of_mut!(n3g_c::opts), priv_data) != 0 {
                return Err(Error::new(ErrorKind::Other, "ntfs3g mkntfs error"));
            }
        }

        #[cfg(not(any(target_arch = "arm", target_arch = "aarch64")))]
        let vol = unsafe { n3g_c::ntfs_mount(dev_name.as_ptr() as *mut i8, 0, priv_data) };
        #[cfg(any(target_arch = "arm", target_arch = "aarch64"))]
        let vol = unsafe { n3g_c::ntfs_mount(dev_name.as_ptr() as *mut u8, 0, priv_data) };
        if vol.is_null() {
            Err(Error::new(
                ErrorKind::Other,
                format!("ntfs3g ntfs_mount error ({})", Error::last_os_error()),
            ))
        } else {
            Ok(Ntfs3g {
                vol,
                inner_type: PhantomData,
            })
        }
    }

    pub fn into_inner(self) -> Result<T> {
        let inner: Box<Box<T>> =
            unsafe { Box::from_raw((*(*self.vol).dev).d_private as *mut Box<T>) };
        if unsafe { n3g_c::ntfs_umount(self.vol, n3g_c::BOOL_TRUE) } != 0 {
            Err(Error::new(
                ErrorKind::Other,
                format!("ntfs3g ntfs_umount error ({})", Error::last_os_error()),
            ))
        } else {
            Ok(*(*inner))
        }
    }

    fn inode_from_path(&self, path: &str) -> Result<*mut n3g_c::ntfs_inode> {
        let path_c = CString::new(path).unwrap();
        let ni: *mut n3g_c::ntfs_inode = unsafe {
            n3g_c::ntfs_pathname_to_inode(&mut *self.vol, std::ptr::null_mut(), path_c.as_ptr())
        };
        if ni.is_null() {
            Err(Error::new(
                ErrorKind::Other,
                format!(
                    "ntfs3g ntfs_pathname_to_inode error ({})",
                    Error::last_os_error()
                ),
            ))
        } else {
            Ok(ni)
        }
    }

    /// Create a new file and returns an NtfsAttr (that impl Write and Seek)
    pub fn new_file<'a>(&'a mut self, path: &str, timestamp: i64) -> Result<NtfsAttr<'a, T>> {
        let (parent_dir, filename) = split_path_parent(path)?;
        let p_ni = self.inode_from_path(&parent_dir)?;
        let path_u16 = str2ntfsunicode(&filename)?;

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
            return Err(Error::new(
                ErrorKind::Other,
                format!("ntfs3g ntfs_create error ({})", Error::last_os_error()),
            ));
        }

        let file_na = unsafe {
            n3g_c::ntfs_attr_open(file_ni, n3g_c::ATTR_TYPES_AT_DATA, std::ptr::null_mut(), 0)
        };
        if file_na.is_null() {
            return Err(Error::new(
                ErrorKind::Other,
                format!("ntfs3g ntfs_attr_open error ({})", Error::last_os_error()),
            ));
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

    pub fn new_dir(&mut self, path: &str, timestamp: i64) -> Result<()> {
        let (parent_dir, filename) = split_path_parent(path)?;
        let p_ni = self.inode_from_path(&parent_dir)?;
        let path_u16 = str2ntfsunicode(&filename)?;

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
                return Err(Error::new(
                    ErrorKind::Other,
                    format!("ntfs3g ntfs_create error ({})", Error::last_os_error()),
                ));
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

    pub fn remove_file(&mut self, path: &str) -> Result<()> {
        let (parent_dir, filename) = split_path_parent(path)?;
        let p_ni = self.inode_from_path(&parent_dir)?;
        let ni = self.inode_from_path(path)?;
        let path_u16 = str2ntfsunicode(&filename)?;
        let path_c = CString::new(filename)
            .map_err(|err| Error::new(ErrorKind::Other, format!("ntfs cstring error ({err})")))?;

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
            return Err(Error::new(
                ErrorKind::Other,
                format!("ntfs3g ntfs_delete error ({})", Error::last_os_error()),
            ));
        }
        unsafe { n3g_c::ntfs_inode_close(p_ni) };
        Ok(())
    }
}

pub struct NtfsAttr<'a, T: Read + Write + Seek> {
    pub attr: *mut n3g_c::ntfs_attr,
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
            Err(io::Error::new(ErrorKind::Other, "NtfsAttr Write error"))
        }
    }

    fn flush(&mut self) -> Result<()> {
        if unsafe { n3g_c::ntfs_inode_sync((*self.attr).ni) } == 0 {
            Ok(())
        } else {
            Err(Error::new(ErrorKind::Other, "ntfs inode sync error"))
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
                    return Err(Error::new(
                        ErrorKind::Other,
                        "ntfs inode seek error from start",
                    ));
                }
            }
            SeekFrom::End(end) => {
                if end <= data_size {
                    self.pos = (data_size - end) as u64;
                } else {
                    return Err(Error::new(
                        ErrorKind::Other,
                        "ntfs inode seek error from end",
                    ));
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
                    return Err(Error::new(
                        ErrorKind::Other,
                        "ntfs inode seek error from cur",
                    ));
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
