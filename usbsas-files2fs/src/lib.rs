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
    os::unix::io::AsRawFd,
};
use thiserror::Error;
use usbsas_comm::{ComRpWriteFs, ProtoRespCommon, ProtoRespWriteFs, ToFromFd};
use usbsas_fsrw::{ff, ntfs, FSWrite};
use usbsas_mbr::SECTOR_START;
use usbsas_proto as proto;
use usbsas_proto::{
    common::{FileType, OutFsType},
    writefs::request::Msg,
};
use usbsas_utils::SECTOR_SIZE;

mod sparsefile;
use sparsefile::{FileBitVec, SparseFile};

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("{0}")]
    FSError(String),
    #[error("fsrw: {0}")]
    Fsrw(#[from] usbsas_fsrw::Error),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
pub type Result<T> = std::result::Result<T, Error>;

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
    fn run(self, comm: &mut ComRpWriteFs) -> Result<Self> {
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
    fn run(self, comm: &mut ComRpWriteFs) -> Result<State> {
        let fs = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.fs_fname)?;
        usbsas_sandbox::files2fs::seccomp(comm.input_fd(), comm.output_fd(), fs.as_raw_fd())?;
        Ok(State::WaitFsInfos(WaitFsInfosState { fs }))
    }
}

impl WaitFsInfosState {
    fn run(self, comm: &mut ComRpWriteFs) -> Result<State> {
        trace!("wait fs infos");
        let newstate = match comm.recv_req()? {
            Msg::SetFsInfos(fsinfos) => match self.mkfs(comm, fsinfos.dev_size, fsinfos.fstype) {
                Ok(fs) => State::WaitNewFile(WaitNewFileState { fs }),
                Err(err) => {
                    comm.error(format!("Error mkfs: {err}"))?;
                    State::WaitEnd(WaitEndState {})
                }
            },
            Msg::ImgDisk(_) => return Ok(State::ImgDisk(ImgDiskState { fs: self.fs })),
            Msg::End(_) => {
                comm.end()?;
                State::End
            }
            _ => {
                error!("bad request");
                comm.error("bad request")?;
                return Err(Error::State);
            }
        };
        Ok(newstate)
    }

    fn mkfs(
        self,
        comm: &mut ComRpWriteFs,
        dev_size: u64,
        fstype: i32,
    ) -> Result<Box<dyn FSWrite<StreamSlice<SparseFile<File>>>>> {
        let out_fs_type =
            OutFsType::try_from(fstype).map_err(|err| Error::FSError(format!("{err}")))?;

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
    fn run(self, comm: &mut ComRpWriteFs) -> Result<State> {
        trace!("wait new file state");
        let newstate = match comm.recv_req()? {
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
                comm.end()?;
                State::End
            }
            _ => {
                error!("bad request");
                comm.error("bad request")?;
                return Err(Error::State);
            }
        };
        Ok(newstate)
    }

    fn newfile(
        mut self,
        comm: &mut ComRpWriteFs,
        path: String,
        timestamp: i64,
        ftype: i32,
    ) -> Result<State> {
        debug!("New file: \"{}\"", &path);
        let newstate: State = match FileType::try_from(ftype) {
            Ok(FileType::Regular) => State::WritingFile(WritingFileState {
                fs: self.fs,
                path,
                timestamp,
            }),
            Ok(FileType::Directory) => {
                match self.fs.newdir(&path, timestamp) {
                    Ok(_) => comm.newfile(proto::writefs::ResponseNewFile {})?,
                    Err(err) => {
                        warn!("{}", err);
                        comm.error(err)?;
                    }
                }
                State::WaitNewFile(self)
            }
            _ => {
                comm.error("bad file type")?;
                return Err(Error::FSError("bad file type".into()));
            }
        };
        Ok(newstate)
    }
}

impl WritingFileState {
    fn run(mut self, comm: &mut ComRpWriteFs) -> Result<State> {
        if let Err(err) = self.write_file(comm) {
            error!("Error writing file: {}", err);
            comm.error(&err)?;
            if let Error::State = err {
                return Ok(State::WaitEnd(WaitEndState {}));
            }
        }

        Ok(State::WaitNewFile(WaitNewFileState { fs: self.fs }))
    }

    fn write_file(&mut self, comm: &mut ComRpWriteFs) -> Result<()> {
        trace!("writing file state");
        let mut file = self.fs.newfile(&self.path, self.timestamp)?;
        comm.newfile(proto::writefs::ResponseNewFile {})?;
        loop {
            match comm.recv_req()? {
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
    fn run(mut self, comm: &mut ComRpWriteFs) -> Result<State> {
        comm.imgdisk(proto::writefs::ResponseImgDisk {})?;
        loop {
            match comm.recv_req()? {
                Msg::WriteData(req) => self.write_data(comm, req.data)?,
                Msg::End(_) => {
                    drop(self.fs);
                    comm.end()?;
                    break;
                }
                _ => {
                    error!("bad request");
                    comm.error("bad request")?;
                    return Err(Error::State);
                }
            }
        }
        Ok(State::End)
    }
    fn write_data(&mut self, comm: &mut ComRpWriteFs, data: Vec<u8>) -> Result<()> {
        self.fs.write_all(&data)?;
        comm.writedata(proto::writefs::ResponseWriteData {})?;
        Ok(())
    }
}

impl ForwardBitVecState {
    fn run(self, comm: &mut ComRpWriteFs) -> Result<State> {
        trace!("forward bitvec state");
        let mut last = false;
        let mut chunks = self.bitvec.chunks(10 * 1024 * 1024).peekable(); // limit protobuf messages to 10Mb
        let newstate = loop {
            match comm.recv_req()? {
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
                    comm.end()?;
                    break State::End;
                }
                _ => {
                    error!("bad request");
                    comm.error("bad request")?;
                    return Err(Error::State);
                }
            }
        };
        Ok(newstate)
    }
}

impl WaitEndState {
    fn run(self, comm: &mut ComRpWriteFs) -> Result<State> {
        trace!("wait end state");
        match comm.recv_req()? {
            Msg::End(_) => {
                comm.end()?;
            }
            _ => {
                error!("bad request");
                comm.error("bad request")?;
            }
        }
        Ok(State::End)
    }
}

pub struct Files2Fs {
    comm: ComRpWriteFs,
    state: State,
}

impl Files2Fs {
    pub fn new(comm: ComRpWriteFs, fs_fname: String) -> Result<Self> {
        let state = State::Init(InitState { fs_fname });
        Ok(Files2Fs { comm, state })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}", err);
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}
