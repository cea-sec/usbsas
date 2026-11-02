use crate::{ReadWriteSeek, n3g_c};
use std::{
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
    let pos = match whence {
        libc::SEEK_SET => match u64::try_from(pos) {
            Ok(pos) => SeekFrom::Start(pos),
            Err(_) => return fail(libc::EINVAL),
        },
        // used by mkntfs to skip bad sectors
        libc::SEEK_CUR => SeekFrom::Current(pos),
        libc::SEEK_END => SeekFrom::End(pos),
        _ => return fail(libc::EINVAL),
    };
    match stream.seek(pos) {
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

/// Read up to `count` bytes, like read(2): returns the number of bytes read
/// (less than `count` only at the end of the stream) or -1.
///
/// # Safety
///
/// See `stream()`. `buf` must be valid for writes of `count` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ntfs_dev_read(
    dev: *mut n3g_c::ntfs_device,
    buf: *mut c_void,
    count: u64,
) -> i64 {
    let Some(stream) = (unsafe { stream(dev) }) else {
        return fail(libc::EINVAL);
    };
    // ntfs-3g passes a signed count: a negative one would show up here as a
    // huge value. Also bound it so the byte count fits in the return value.
    let Some(count) = usize::try_from(count)
        .ok()
        .filter(|&count| i64::try_from(count).is_ok() && isize::try_from(count).is_ok())
    else {
        return fail(libc::EINVAL);
    };
    if count == 0 {
        return 0;
    }
    if buf.is_null() {
        return fail(libc::EFAULT);
    }

    // buf may be uninitialized memory: zero it first so that a &mut [u8]
    // over it is valid, then read in place.
    let buf = buf as *mut u8;
    unsafe { std::ptr::write_bytes(buf, 0, count) };
    let data = unsafe { std::slice::from_raw_parts_mut(buf, count) };

    let mut total = 0;
    while total < count {
        match stream.read(&mut data[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("ntfs read error: {err}");
                return fail(io_errno(&err));
            }
        }
    }
    // total <= count which fits in i64 (checked above)
    total as i64
}

/// Write `count` bytes, returns `count` or -1.
///
/// # Safety
///
/// See `stream()`. `buf` must be valid for reads of `count` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ntfs_dev_write(
    dev: *mut n3g_c::ntfs_device,
    buf: *const c_void,
    count: u64,
) -> i64 {
    let Some(stream) = (unsafe { stream(dev) }) else {
        return fail(libc::EINVAL);
    };
    let Some(count) = usize::try_from(count)
        .ok()
        .filter(|&count| i64::try_from(count).is_ok() && isize::try_from(count).is_ok())
    else {
        return fail(libc::EINVAL);
    };
    if count == 0 {
        return 0;
    }
    if buf.is_null() {
        return fail(libc::EFAULT);
    }

    let data = unsafe { std::slice::from_raw_parts(buf as *const u8, count) };
    if let Err(err) = stream.write_all(data) {
        eprintln!("ntfs write error: {err}");
        return fail(io_errno(&err));
    }
    count as i64
}
