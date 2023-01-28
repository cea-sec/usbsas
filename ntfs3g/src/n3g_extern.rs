use crate::n3g_c;
use std::{
    convert::TryFrom,
    io::{Read, Seek, SeekFrom, Write},
    os::raw::{c_int, c_void},
};

// impl ntfs3g extern functions defined in unix_io.h

#[no_mangle]
pub extern "C" fn ntfs_dev_lseek(dev: *mut n3g_c::ntfs_device, pos: i64, whence: c_int) -> i64 {
    if whence != libc::SEEK_SET {
        return -1;
    }
    if pos < 0 {
        return -1;
    }

    let mut inner: Box<Box<dyn crate::ReadWriteSeek>> =
        unsafe { Box::from_raw((*dev).d_private as *mut Box<dyn crate::ReadWriteSeek>) };

    let seek_ret = inner.seek(SeekFrom::Start(pos as u64));
    std::mem::forget(inner);

    match seek_ret {
        Ok(new_pos) => i64::try_from(new_pos).unwrap_or(-1),
        Err(err) => {
            eprintln!("ntfs seek error: {err}");
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn ntfs_dev_read(
    dev: *mut n3g_c::ntfs_device,
    buf: *mut c_void,
    count: usize,
) -> i64 {
    let mut inner: Box<Box<dyn crate::ReadWriteSeek>> =
        unsafe { Box::from_raw((*dev).d_private as *mut Box<dyn crate::ReadWriteSeek>) };

    let mut data = vec![0; count];

    let read_ret = inner.read_exact(&mut data);
    std::mem::forget(inner);

    if let Err(err) = read_ret {
        eprintln!("ntfs read error: {err}");
        return -1;
    };

    if data.is_empty() {
        return 0;
    }

    unsafe { std::ptr::copy(data.as_mut_ptr(), buf as *mut u8, data.len()) };

    if let Ok(count) = i64::try_from(count) {
        count
    } else {
        -1
    }
}

#[no_mangle]
pub extern "C" fn ntfs_dev_write(
    dev: *mut n3g_c::ntfs_device,
    buf: *mut c_void,
    count: usize,
) -> i64 {
    let mut inner: Box<Box<dyn crate::ReadWriteSeek>> =
        unsafe { Box::from_raw((*dev).d_private as *mut Box<dyn crate::ReadWriteSeek>) };

    let slice = unsafe { std::slice::from_raw_parts(buf as *mut u8, count) };

    let write_ret = inner.write_all(slice);
    std::mem::forget(inner);

    if let Err(err) = write_ret {
        eprintln!("ntfs write error: {err}");
        return -1;
    }

    if let Ok(count) = i64::try_from(count) {
        count
    } else {
        -1
    }
}
