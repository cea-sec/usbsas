//! files2fs writes files in a new filesystem with partition table on disk (not
//! on the destination USB device directly, that's fs2dev's job). Supported file
//! systems are `FAT`, `exFAT` and `NTFS`. The size of the created file system
//! is the size of the destination USB device. When writing the file system,
//! files2fs will keep track of the (non empty) sectors actually written in a
//! bit vector, fs2dev will use this bit vector to avoid writing the whole file
//! system on the destination device.

use fscommon::StreamSlice;
use log::{debug, error, trace, warn};
use std::{
    convert::TryFrom,
    fs::{self, File},
    io::{Seek, SeekFrom, Write},
    os::unix::io::{AsRawFd, RawFd},
};
use thiserror::Error;
use usbsas_comm::{protoresponse, Comm};
use usbsas_fsrw::{ff, ntfs, FSWrite};
use usbsas_mbr::SECTOR_START;
use usbsas_process::UsbsasProcess;
use usbsas_proto as proto;
use usbsas_proto::{
    common::{FileType, OutFsType},
    writefs::request::Msg,
};
use usbsas_utils::SECTOR_SIZE;

mod sparsefile;
use sparsefile::{FileBitVec, SparseFile};

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("{0}")]
    FSError(String),
    #[error("privileges: {0}")]
    Fsrw(#[from] usbsas_fsrw::Error),
    #[error("privileges: {0}")]
    Privileges(#[from] usbsas_privileges::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
type Result<T> = std::result::Result<T, Error>;

protoresponse!(
    CommWritefs,
    writefs,
    setfsinfos = SetFsInfos[ResponseSetFsInfos],
    newfile = NewFile[ResponseNewFile],
    writefile = WriteFile[ResponseWriteFile],
    endfile = EndFile[ResponseEndFile],
    imgdisk = ImgDisk[ResponseImgDisk],
    writedata = WriteData[ResponseWriteData],
    close = Close[ResponseClose],
    bitvec = BitVec[ResponseBitVec],
    error = Error[ResponseError],
    end = End[ResponseEnd]
);

enum State {
    Init(InitState),
    WaitFsInfos(WaitFsInfosState),
    WaitNewFile(WaitNewFileState),
    WritingFile(WritingFileState),
    ImgDisk(ImgDiskState),
    ForwardBitVec(ForwardBitVecState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut Comm<proto::writefs::Request>) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::WaitFsInfos(s) => s.run(comm),
            State::WaitNewFile(s) => s.run(comm),
            State::WritingFile(s) => s.run(comm),
            State::WaitEnd(s) => s.run(comm),
            State::ImgDisk(s) => s.run(comm),
            State::ForwardBitVec(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {
    fs_fname: String,
}

struct WaitFsInfosState {
    fs: File,
}

struct WaitNewFileState {
    fs: Box<dyn FSWrite<StreamSlice<SparseFile<File>>>>,
}

struct WritingFileState {
    fs: Box<dyn FSWrite<StreamSlice<SparseFile<File>>>>,
    path: String,
    timestamp: i64,
}

struct ImgDiskState {
    fs: File,
}

struct ForwardBitVecState {
    bitvec: FileBitVec,
}

struct WaitEndState;

impl InitState {
    fn run(self, comm: &mut Comm<proto::writefs::Request>) -> Result<State> {
        let fs = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.fs_fname)?;
        usbsas_privileges::files2fs::drop_priv(comm.input_fd(), comm.output_fd(), fs.as_raw_fd())?;
        Ok(State::WaitFsInfos(WaitFsInfosState { fs }))
    }
}

impl WaitFsInfosState {
    fn run(self, comm: &mut Comm<proto::writefs::Request>) -> Result<State> {
        trace!("wait fs infos");
        let req: proto::writefs::Request = comm.recv()?;
        let newstate = match req.msg.ok_or(Error::BadRequest)? {
            Msg::SetFsInfos(fsinfos) => match self.mkfs(comm, fsinfos.dev_size, fsinfos.fstype) {
                Ok(fs) => State::WaitNewFile(WaitNewFileState { fs }),
                Err(err) => {
                    comm.error(proto::writefs::ResponseError {
                        err: format!("Error mkfs: {}", err),
                    })?;
                    State::WaitEnd(WaitEndState {})
                }
            },
            Msg::ImgDisk(_) => return Ok(State::ImgDisk(ImgDiskState { fs: self.fs })),
            Msg::End(_) => {
                comm.end(proto::writefs::ResponseEnd {})?;
                State::End
            }
            _ => {
                error!("bad request");
                comm.error(proto::writefs::ResponseError {
                    err: "bad request".into(),
                })?;
                return Err(Error::State);
            }
        };
        Ok(newstate)
    }

    fn mkfs(
        self,
        comm: &mut Comm<proto::writefs::Request>,
        dev_size: u64,
        fstype: i32,
    ) -> Result<Box<dyn FSWrite<StreamSlice<SparseFile<File>>>>> {
        let out_fs_type =
            OutFsType::from_i32(fstype).ok_or_else(|| Error::FSError("bad fstype".into()))?;

        log::debug!("mkfs dev_size: {}", dev_size);

        let fs_size = dev_size - (SECTOR_START * SECTOR_SIZE);
        if fs_size % SECTOR_SIZE != 0 {
            return Err(Error::FSError("fs size not multiple of sector size".into()));
        }

        if dev_size % SECTOR_SIZE != 0 {
            return Err(Error::FSError(
                "dev size not multiple of sector size".into(),
            ));
        }
        let sector_count = fs_size / SECTOR_SIZE;
        let sector_count: u64 = if sector_count > 0xFFFF_FFFF {
            return Err(Error::FSError("sector count too big".into()));
        } else {
            sector_count & 0xFFFF_FFFF
        };

        let mut sparse_file =
            SparseFile::new(self.fs, SECTOR_SIZE, (SECTOR_START + sector_count) as usize)?;

        let fs: Box<dyn FSWrite<StreamSlice<SparseFile<File>>>> = match out_fs_type {
            OutFsType::Fat | OutFsType::Exfat => {
                // ff handles writing mbr but still wrap in StreamSlice so we have the same type as ntfs below
                let file_slice =
                    StreamSlice::new(sparse_file, 0, (SECTOR_START + sector_count) * SECTOR_SIZE)?;

                Box::new(ff::FatFsWriter::mkfs(
                    file_slice,
                    SECTOR_SIZE,
                    sector_count,
                    Some(out_fs_type),
                )?)
            }
            OutFsType::Ntfs => {
                // Write mbr before mkfs
                sparse_file.seek(SeekFrom::Start(446))?;
                let partition = usbsas_mbr::MbrPartitionEntry {
                    boot_indicator: 0,
                    start_head: 1,
                    start_sector: 1,
                    start_cylinder: 0,
                    partition_type: 0x7,
                    end_head: 0xfe,
                    end_sector: 0x3f,
                    end_cylinder: 0x2,
                    start_in_lba: u32::try_from(SECTOR_START)?,
                    size_in_lba: u32::try_from(sector_count)?,
                };
                usbsas_mbr::write_partition(&mut sparse_file, &partition)?;
                sparse_file.seek(SeekFrom::Start(510))?;
                sparse_file.write_all(&[0x55, 0xAA])?;

                let file_slice = StreamSlice::new(
                    sparse_file,
                    SECTOR_START * SECTOR_SIZE,
                    (SECTOR_START + sector_count) * SECTOR_SIZE,
                )?;

                Box::new(ntfs::NTFS3G::mkfs(
                    file_slice,
                    SECTOR_SIZE,
                    sector_count,
                    None,
                )?)
            }
        };

        comm.setfsinfos(proto::writefs::ResponseSetFsInfos {})?;
        Ok(fs)
    }
}

impl WaitNewFileState {
    fn run(self, comm: &mut Comm<proto::writefs::Request>) -> Result<State> {
        trace!("wait new file state");
        let req: proto::writefs::Request = comm.recv()?;
        let newstate = match req.msg.ok_or(Error::BadRequest)? {
            Msg::NewFile(msg) => self.newfile(comm, msg.path, msg.timestamp, msg.ftype)?,
            Msg::Close(_) => {
                let bitvec = self.fs.unmount_fs()?.into_inner().get_bitvec()?;
                comm.close(proto::writefs::ResponseClose {})?;
                State::ForwardBitVec(ForwardBitVecState { bitvec })
            }
            Msg::EndFile(_) => {
                comm.endfile(proto::writefs::ResponseEndFile {})?;
                State::WaitNewFile(self)
            }
            Msg::End(_) => {
                let _ = self.fs.unmount_fs()?;
                comm.end(proto::writefs::ResponseEnd {})?;
                State::End
            }
            _ => {
                error!("bad request");
                comm.error(proto::writefs::ResponseError {
                    err: "bad request".into(),
                })?;
                return Err(Error::State);
            }
        };
        Ok(newstate)
    }

    fn newfile(
        mut self,
        comm: &mut Comm<proto::writefs::Request>,
        path: String,
        timestamp: i64,
        ftype: i32,
    ) -> Result<State> {
        debug!("New file: \"{}\"", &path);
        let newstate: State = match FileType::from_i32(ftype) {
            Some(FileType::Regular) => State::WritingFile(WritingFileState {
                fs: self.fs,
                path,
                timestamp,
            }),
            Some(FileType::Directory) => {
                match self.fs.newdir(&path, timestamp) {
                    Ok(_) => comm.newfile(proto::writefs::ResponseNewFile {})?,
                    Err(err) => {
                        warn!("{}", err);
                        comm.error(proto::writefs::ResponseError {
                            err: format!("{}", err),
                        })?;
                    }
                }
                State::WaitNewFile(self)
            }
            _ => {
                comm.error(proto::writefs::ResponseError {
                    err: "bad file type".into(),
                })?;
                return Err(Error::FSError("bad file type".into()));
            }
        };
        Ok(newstate)
    }
}

impl WritingFileState {
    fn run(mut self, comm: &mut Comm<proto::writefs::Request>) -> Result<State> {
        if let Err(err) = self.write_file(comm) {
            error!("Error writing file: {}", err);
            comm.error(proto::writefs::ResponseError {
                err: format!("{}", err),
            })?;
            if let Error::State = err {
                return Ok(State::WaitEnd(WaitEndState {}));
            }
        }

        Ok(State::WaitNewFile(WaitNewFileState { fs: self.fs }))
    }

    fn write_file(&mut self, comm: &mut Comm<proto::writefs::Request>) -> Result<()> {
        trace!("writing file state");
        let mut file = self.fs.newfile(&self.path, self.timestamp)?;
        comm.newfile(proto::writefs::ResponseNewFile {})?;
        loop {
            let req: proto::writefs::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::WriteFile(msg) => {
                    if file.seek(SeekFrom::End(0))? != msg.offset {
                        error!("sparse write not supported");
                        // drop to close file
                        drop(file);
                        self.fs.removefile(&self.path)?;
                        return Err(Error::FSError("sparse write not supported".into()));
                    }
                    if let Err(err) = file.write_all(&msg.data) {
                        error!("Error writing file: {}, deleting file", err);
                        // drop to close file
                        drop(file);
                        self.fs.removefile(&self.path)?;
                        return Err(Error::FSError("err writing file".into()));
                    }
                    comm.writefile(proto::writefs::ResponseWriteFile {})?;
                }
                Msg::EndFile(_) => {
                    drop(file);
                    self.fs.settimestamp(&self.path, self.timestamp)?;
                    comm.endfile(proto::writefs::ResponseEndFile {})?;
                    break;
                }
                _ => {
                    return Err(Error::State);
                }
            }
        }

        Ok(())
    }
}

impl ImgDiskState {
    fn run(mut self, comm: &mut Comm<proto::writefs::Request>) -> Result<State> {
        comm.imgdisk(proto::writefs::ResponseImgDisk {})?;
        loop {
            let req: proto::writefs::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::WriteData(req) => self.write_data(comm, req.data)?,
                Msg::End(_) => {
                    drop(self.fs);
                    comm.end(proto::writefs::ResponseEnd {})?;
                    break;
                }
                _ => {
                    error!("bad request");
                    comm.error(proto::writefs::ResponseError {
                        err: "bad request".into(),
                    })?;
                    return Err(Error::State);
                }
            }
        }
        Ok(State::End)
    }
    fn write_data(
        &mut self,
        comm: &mut Comm<proto::writefs::Request>,
        data: Vec<u8>,
    ) -> Result<()> {
        self.fs.write_all(&data)?;
        comm.writedata(proto::writefs::ResponseWriteData {})?;
        Ok(())
    }
}

impl ForwardBitVecState {
    fn run(self, comm: &mut Comm<proto::writefs::Request>) -> Result<State> {
        trace!("forward bitvec state");
        let mut last = false;
        let mut chunks = self.bitvec.chunks(10 * 1024 * 1024).peekable(); // limit protobuf messages to 10Mb
        let newstate = loop {
            let req: proto::writefs::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::BitVec(_) => {
                    let chunk = chunks.next().unwrap().to_bitvec().into_vec();
                    if chunks.peek().is_none() {
                        last = true;
                    }
                    comm.bitvec(proto::writefs::ResponseBitVec { chunk, last })?;
                    if last {
                        break State::WaitEnd(WaitEndState);
                    }
                }
                Msg::End(_) => {
                    comm.end(proto::writefs::ResponseEnd {})?;
                    break State::End;
                }
                _ => {
                    error!("bad request");
                    comm.error(proto::writefs::ResponseError {
                        err: "bad request".into(),
                    })?;
                    return Err(Error::State);
                }
            }
        };
        Ok(newstate)
    }
}

impl WaitEndState {
    fn run(self, comm: &mut Comm<proto::writefs::Request>) -> Result<State> {
        trace!("wait end state");
        let req: proto::writefs::Request = comm.recv()?;
        match req.msg.ok_or(Error::BadRequest)? {
            Msg::End(_) => {
                comm.end(proto::writefs::ResponseEnd {})?;
            }
            _ => {
                error!("bad request");
                comm.error(proto::writefs::ResponseError {
                    err: "bad request".into(),
                })?;
            }
        }
        Ok(State::End)
    }
}

pub struct Files2Fs {
    comm: Comm<proto::writefs::Request>,
    state: State,
}

impl Files2Fs {
    fn new(comm: Comm<proto::writefs::Request>, fs_fname: String) -> Result<Self> {
        let state = State::Init(InitState { fs_fname });
        Ok(Files2Fs { comm, state })
    }

    fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}", err);
                    comm.error(proto::writefs::ResponseError {
                        err: format!("run error: {}", err),
                    })?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}

impl UsbsasProcess for Files2Fs {
    fn spawn(
        read_fd: RawFd,
        write_fd: RawFd,
        args: Option<Vec<String>>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        if let Some(args) = args {
            if let Some(fname) = args.get(0) {
                log::info!("files2fs {:?}", fname);
                Files2Fs::new(Comm::from_raw_fd(read_fd, write_fd), fname.to_owned())?
                    .main_loop()
                    .map(|_| log::debug!("files2fs: exiting"))?;
                return Ok(());
            }
        }
        Err(Box::new(Error::Error(
            "files2fs needs a fs filename as arg".to_string(),
        )))
    }
}
