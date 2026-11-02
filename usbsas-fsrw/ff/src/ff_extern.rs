use crate::{ff_c, wrapper::WrapperFatFs};
use std::{io::SeekFrom, os::raw::c_void};

// Impl ff extern functions defined in disk_io.h
//
// `pdrv` is the `FATFS.pdrv` field set by `FatFs` (lib.rs): a thin pointer to a
// heap allocated `Box<dyn WrapperFatFs>` that `FatFs` owns. These callbacks
// only *borrow* it for the duration of the call; ownership never transits
// through here.

/// Borrow the drive behind `pdrv`.
///
/// # Safety
///
/// `pdrv` must be null or the pointer stored in `FATFS.pdrv` by `FatFs`, still
/// alive, and not otherwise borrowed for `'a`. `FatFs` holds it as a raw
/// pointer only, so no Rust reference aliases it while ff runs a callback.
unsafe fn drive<'a>(pdrv: *mut c_void) -> Option<&'a mut (dyn WrapperFatFs + 'a)> {
    unsafe { (pdrv as *mut Box<dyn WrapperFatFs + 'a>).as_mut() }.map(|b| &mut **b)
}

/// Byte length of `count` sectors, if it fits in memory.
fn sectors_len(drive: &dyn WrapperFatFs, count: ff_c::UINT) -> Option<usize> {
    u64::from(count)
        .checked_mul(u64::from(drive.sector_size()))
        .and_then(|len| usize::try_from(len).ok())
}

fn seek_sector(drive: &mut dyn WrapperFatFs, sector: ff_c::DWORD) -> std::io::Result<u64> {
    // u32 * u32 can't overflow u64
    drive.seek(SeekFrom::Start(
        u64::from(sector) * u64::from(drive.sector_size()),
    ))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ff_disk_initialize(_pdrv: *mut c_void) -> ff_c::DSTATUS {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ff_disk_status(_pdrv: *mut c_void) -> ff_c::DSTATUS {
    0
}

/// # Safety
///
/// See `drive()`. `buff` must be valid for writes of `count` sectors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ff_disk_read(
    pdrv: *mut c_void,
    buff: *mut ff_c::BYTE,
    sector: ff_c::DWORD,
    count: ff_c::UINT,
) -> ff_c::DRESULT {
    let Some(drive) = (unsafe { drive(pdrv) }) else {
        return ff_c::DRESULT_RES_PARERR;
    };
    let Some(len) = sectors_len(drive, count) else {
        return ff_c::DRESULT_RES_PARERR;
    };
    if buff.is_null() {
        return ff_c::DRESULT_RES_PARERR;
    }

    if let Err(err) = seek_sector(drive, sector) {
        eprintln!("ff disk_read seek error: {err}");
        return ff_c::DRESULT_RES_ERROR;
    }

    // Read into an owned buffer rather than a slice over `buff`: ff may hand
    // us uninitialized memory (e.g. f_mkfs's malloc'd work buffer), and a
    // `&mut [u8]` over uninitialized memory is UB.
    let mut data = vec![0; len];
    if let Err(err) = drive.read_exact(&mut data) {
        eprintln!("ff disk_read error: {err}");
        return ff_c::DRESULT_RES_ERROR;
    }

    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buff, len) };
    ff_c::DRESULT_RES_OK
}

/// # Safety
///
/// See `drive()`. `buff` must be valid for reads of `count` sectors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ff_disk_write(
    pdrv: *mut c_void,
    buff: *const ff_c::BYTE,
    sector: ff_c::DWORD,
    count: ff_c::UINT,
) -> ff_c::DRESULT {
    let Some(drive) = (unsafe { drive(pdrv) }) else {
        return ff_c::DRESULT_RES_PARERR;
    };
    let Some(len) = sectors_len(drive, count) else {
        return ff_c::DRESULT_RES_PARERR;
    };
    if buff.is_null() {
        return ff_c::DRESULT_RES_PARERR;
    }

    if let Err(err) = seek_sector(drive, sector) {
        eprintln!("ff disk_write seek error: {err}");
        return ff_c::DRESULT_RES_ERROR;
    }

    let slice = unsafe { std::slice::from_raw_parts(buff, len) };
    if let Err(err) = drive.write_all(slice) {
        eprintln!("ff disk_write error: {err}");
        return ff_c::DRESULT_RES_ERROR;
    }
    ff_c::DRESULT_RES_OK
}

/// # Safety
///
/// See `drive()`. `buff` must be valid for the data type implied by `cmd`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ff_disk_ioctl(
    pdrv: *mut c_void,
    cmd: ff_c::BYTE,
    buff: *mut c_void,
) -> ff_c::DRESULT {
    let Some(drive) = (unsafe { drive(pdrv) }) else {
        return ff_c::DRESULT_RES_PARERR;
    };
    match u32::from(cmd) {
        ff_c::CTRL_SYNC => {
            if let Err(err) = drive.flush() {
                eprintln!("ff ioctl CTRL_SYNC failed: {err}");
                ff_c::DRESULT_RES_ERROR
            } else {
                ff_c::DRESULT_RES_OK
            }
        }
        ff_c::GET_SECTOR_SIZE => {
            if buff.is_null() {
                return ff_c::DRESULT_RES_PARERR;
            }
            // sector_size is checked to be <= 4096 when FatFs is created
            unsafe { (buff as *mut ff_c::WORD).write_unaligned(drive.sector_size() as u16) };
            ff_c::DRESULT_RES_OK
        }
        _ => {
            eprintln!("unsupported ioctl cmd: {cmd}");
            ff_c::DRESULT_RES_ERROR
        }
    }
}
