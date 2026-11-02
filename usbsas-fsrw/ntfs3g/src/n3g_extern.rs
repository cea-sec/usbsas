use crate::{ReadWriteSeek, n3g_c};
use std::{
    convert::TryFrom,
    io::SeekFrom,
    os::raw::{c_int, c_void},
};

// impl ntfs3g extern functions defined in unix_io.h
//
// `dev.d_private` is the pointer set by `Ntfs3g` (lib.rs): a thin pointer to a
// heap allocated `Box<dyn ReadWriteSeek>` that `Ntfs3g` owns. These callbacks
// only *borrow* it for the duration of the call; ownership never transits
// through here.

/// Borrow the stream behind `dev`.
///
/// # Safety
///
/// `dev` must be null or a valid `ntfs_device` whose `d_private` is null or
/// the pointer set by `Ntfs3g`, still alive and not otherwise borrowed for
/// `'a`.
unsafe fn stream<'a>(dev: *mut n3g_c::ntfs_device) -> Option<&'a mut (dyn ReadWriteSeek + 'a)> {
    let dev = unsafe { dev.as_ref() }?;
    unsafe { (dev.d_private as *mut Box<dyn ReadWriteSeek + 'a>).as_mut() }.map(|b| &mut **b)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ntfs_dev_lseek(
    dev: *mut n3g_c::ntfs_device,
    pos: i64,
    whence: c_int,
) -> i64 {
    if whence != libc::SEEK_SET {
        return -1;
    }
    if pos < 0 {
        return -1;
    }
    let Some(stream) = (unsafe { stream(dev) }) else {
        return -1;
    };

    match stream.seek(SeekFrom::Start(pos as u64)) {
        Ok(new_pos) => i64::try_from(new_pos).unwrap_or(-1),
        Err(err) => {
            eprintln!("ntfs seek error: {err}");
            -1
        }
    }
}

/// # Safety
///
/// See `stream()`. `buf` must be valid for writes of `count` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ntfs_dev_read(
    dev: *mut n3g_c::ntfs_device,
    buf: *mut c_void,
    count: usize,
) -> i64 {
    let Some(stream) = (unsafe { stream(dev) }) else {
        return -1;
    };

    let mut data = vec![0; count];

    if let Err(err) = stream.read_exact(&mut data) {
        eprintln!("ntfs read error: {err}");
        return -1;
    };

    if data.is_empty() {
        return 0;
    }

    unsafe { std::ptr::copy(data.as_mut_ptr(), buf as *mut u8, data.len()) };

    i64::try_from(count).unwrap_or(-1)
}

/// # Safety
///
/// See `stream()`. `buf` must be valid for reads of `count` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ntfs_dev_write(
    dev: *mut n3g_c::ntfs_device,
    buf: *mut c_void,
    count: usize,
) -> i64 {
    let Some(stream) = (unsafe { stream(dev) }) else {
        return -1;
    };

    let slice = unsafe { std::slice::from_raw_parts(buf as *mut u8, count) };

    if let Err(err) = stream.write_all(slice) {
        eprintln!("ntfs write error: {err}");
        return -1;
    }
    i64::try_from(count).unwrap_or(-1)
}
