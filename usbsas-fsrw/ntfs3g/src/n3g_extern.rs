use crate::{ReadWriteSeek, n3g_c};
use std::{
    convert::TryFrom,
    io::{ErrorKind, SeekFrom},
    os::raw::{c_int, c_void},
};

// impl ntfs3g extern functions defined in unix_io.h
//
// `dev.d_private` is the pointer set by `Ntfs3g` (lib.rs): a thin pointer to a
// heap allocated `Box<dyn ReadWriteSeek>` that `Ntfs3g` owns. These callbacks
// only *borrow* it for the duration of the call; ownership never transits
// through here.
//
// On error they return -1 *and set errno*: ntfs-3g decides what to do from
// errno (e.g. it retries while errno == EINTR, and mkntfs expects EIO), so
// leaving a stale value could make it loop or misbehave.

/// Borrow the stream behind `dev`.
///
/// # Safety
///
/// `dev` must be null or a valid `ntfs_device` whose `d_private` is null or
/// the pointer set by `Ntfs3g`, still alive and not otherwise borrowed for
/// `'a`. `Ntfs3g` holds it as a raw pointer only, so no Rust reference aliases
/// it while ntfs-3g runs a callback.
unsafe fn stream<'a>(dev: *mut n3g_c::ntfs_device) -> Option<&'a mut (dyn ReadWriteSeek + 'a)> {
    let dev = unsafe { dev.as_ref() }?;
    unsafe { (dev.d_private as *mut Box<dyn ReadWriteSeek + 'a>).as_mut() }.map(|b| &mut **b)
}

fn set_errno(errno: c_int) {
    unsafe { *libc::__errno_location() = errno };
}

/// Return -1 with errno set
fn fail(errno: c_int) -> i64 {
    set_errno(errno);
    -1
}

fn io_errno(err: &std::io::Error) -> c_int {
    // never report EINTR / EAGAIN: ntfs-3g would retry forever
    match err.raw_os_error() {
        Some(libc::EINTR) | Some(libc::EAGAIN) | None => libc::EIO,
        Some(errno) => errno,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ntfs_dev_lseek(
    dev: *mut n3g_c::ntfs_device,
    pos: i64,
    whence: c_int,
) -> i64 {
    let Some(stream) = (unsafe { stream(dev) }) else {
        return fail(libc::EINVAL);
    };
    if whence != libc::SEEK_SET {
        return fail(libc::EINVAL);
    }
    let Ok(pos) = u64::try_from(pos) else {
        return fail(libc::EINVAL);
    };
    match stream.seek(SeekFrom::Start(pos)) {
        Ok(new_pos) => i64::try_from(new_pos).unwrap_or_else(|_| fail(libc::EOVERFLOW)),
        Err(err) => {
            eprintln!("ntfs seek error: {err}");
            fail(match err.kind() {
                ErrorKind::InvalidInput => libc::EINVAL,
                _ => io_errno(&err),
            })
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
        return fail(libc::EINVAL);
    };

    let mut data = vec![0; count];

    if let Err(err) = stream.read_exact(&mut data) {
        eprintln!("ntfs read error: {err}");
        return fail(io_errno(&err));
    };

    if data.is_empty() {
        return 0;
    }

    unsafe { std::ptr::copy(data.as_mut_ptr(), buf as *mut u8, data.len()) };

    i64::try_from(count).unwrap_or_else(|_| fail(libc::EOVERFLOW))
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
        return fail(libc::EINVAL);
    };

    let slice = unsafe { std::slice::from_raw_parts(buf as *mut u8, count) };

    if let Err(err) = stream.write_all(slice) {
        eprintln!("ntfs write error: {err}");
        return fail(io_errno(&err));
    }
    i64::try_from(count).unwrap_or_else(|_| fail(libc::EOVERFLOW))
}
