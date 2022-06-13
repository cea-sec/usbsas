#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]
#![allow(clippy::upper_case_acronyms)]
#![allow(dead_code)]
#![allow(non_snake_case)]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

impl FATFS {
    pub(crate) fn new() -> Self {
        FATFS {
            fs_type: 0,
            pdrv: std::ptr::null_mut(),
            n_fats: 0,
            wflag: 0,
            fsi_flag: 0,
            id: 0,
            n_rootdir: 0,
            csize: 0,
            ssize: 0,
            lfnbuf: std::ptr::null_mut(),
            dirbuf: std::ptr::null_mut(),
            last_clst: 0,
            free_clst: 0,
            n_fatent: 0,
            fsize: 0,
            volbase: 0,
            fatbase: 0,
            dirbase: 0,
            database: 0,
            bitbase: 0,
            winsect: 0,
            win: [0; 4096usize],
        }
    }
}

impl FILINFO {
    pub(crate) fn new() -> Self {
        FILINFO {
            fsize: 0,
            fdate: 0,
            ftime: 0,
            fattrib: 0,
            altname: [0; 13],
            fname: [0; 256],
        }
    }
}

impl DIR {
    pub(crate) fn new() -> Self {
        DIR {
            obj: FFOBJID::new(),
            dptr: 0,
            clust: 0,
            sect: 0,
            dir: std::ptr::null_mut(),
            fn_: [0; 12usize],
            blk_ofs: 0,
        }
    }
}

impl FFOBJID {
    pub(crate) fn new() -> Self {
        FFOBJID {
            fs: std::ptr::null_mut(),
            id: 0,
            attr: 0,
            stat: 0,
            sclust: 0,
            objsize: 0,
            n_cont: 0,
            n_frag: 0,
            c_scl: 0,
            c_size: 0,
            c_ofs: 0,
        }
    }
}

impl FIL {
    pub(crate) fn new() -> Self {
        FIL {
            obj: FFOBJID::new(),
            flag: 0,
            err: 0,
            fptr: 0,
            clust: 0,
            sect: 0,
            dir_sect: 0,
            dir_ptr: std::ptr::null_mut(),
            buf: [0; 4096usize],
        }
    }
}
