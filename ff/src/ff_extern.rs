use crate::{ff_c, wrapper::WrapperFatFs};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    os::raw::c_void,
};

// Impl ff extern functions defined in disk_io.h

#[no_mangle]
pub extern "C" fn ff_disk_initialize(_pdrv: c_void) -> ff_c::DSTATUS {
    0
}

#[no_mangle]
pub extern "C" fn ff_disk_status(_pdrv: c_void) -> ff_c::DSTATUS {
    0
}

#[no_mangle]
pub extern "C" fn ff_disk_read(
    pdrv: *mut c_void,
    buff: *mut ff_c::BYTE,
    sector: ff_c::DWORD,
    count: ff_c::UINT,
) -> ff_c::DRESULT {
    // See fn new() in lib.rs about the double box
    let mut inner: Box<Box<dyn WrapperFatFs>> =
        unsafe { Box::from_raw(pdrv as *mut Box<dyn WrapperFatFs>) };

    if let Err(err) = inner.seek(SeekFrom::Start(sector as u64 * inner.sector_size() as u64)) {
        eprintln!("ff disk_read seek error: {}", err);
        std::mem::forget(inner);
        return ff_c::DRESULT_RES_ERROR;
    }

    let mut data = vec![0; (count as u64 * inner.sector_size() as u64) as usize];
    if let Ok(read_count) = inner.read(&mut data) {
        if read_count != data.len() {
            eprintln!("ff disk_read read_count != data.len()");
            std::mem::forget(inner);
            return ff_c::DRESULT_RES_ERROR;
        }
    };

    unsafe { std::ptr::copy(data.as_mut_ptr(), buff, data.len()) };
    std::mem::forget(inner);
    ff_c::DRESULT_RES_OK
}

#[no_mangle]
pub extern "C" fn ff_disk_write(
    pdrv: *mut c_void,
    buff: *const ff_c::BYTE,
    sector: ff_c::DWORD,
    count: ff_c::UINT,
) -> ff_c::DRESULT {
    let mut inner: Box<Box<dyn WrapperFatFs>> =
        unsafe { Box::from_raw(pdrv as *mut Box<dyn WrapperFatFs>) };

    if let Err(err) = inner.seek(SeekFrom::Start(sector as u64 * inner.sector_size() as u64)) {
        eprintln!("ff disk_write seek error: {}", err);
        std::mem::forget(inner);
        return ff_c::DRESULT_RES_ERROR;
    }
    let slice = unsafe {
        std::slice::from_raw_parts(buff, (count as u64 * inner.sector_size() as u64) as usize)
    };
    if let Err(err) = inner.write_all(slice) {
        eprintln!("ff disk_write error: {}", err);
        std::mem::forget(inner);
        return ff_c::DRESULT_RES_ERROR;
    }
    std::mem::forget(inner);
    ff_c::DRESULT_RES_OK
}

#[no_mangle]
pub extern "C" fn ff_disk_ioctl(
    pdrv: *mut c_void,
    cmd: ff_c::BYTE,
    buff: *mut c_void,
) -> ff_c::DRESULT {
    let mut inner: Box<Box<dyn WrapperFatFs>> =
        unsafe { Box::from_raw(pdrv as *mut Box<dyn WrapperFatFs>) };
    let ret = match cmd as u32 {
        ff_c::CTRL_SYNC => {
            if let Err(err) = inner.flush() {
                eprintln!("ff ioctl CTRL_SYNC failed: {}", err);
                ff_c::DRESULT_RES_ERROR
            } else {
                ff_c::DRESULT_RES_OK
            }
        }
        ff_c::GET_SECTOR_SIZE => {
            unsafe { *(buff as *mut ff_c::WORD) = inner.sector_size() as u16 };
            ff_c::DRESULT_RES_OK
        }
        _ => {
            eprintln!("unsupported ioctl cmd: {}", cmd);
            ff_c::DRESULT_RES_ERROR
        }
    };
    std::mem::forget(inner);
    ret
}
