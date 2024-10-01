//! usbsas process responsible for writing the file system on destination USB
//! device. It can also wipe devices (with 0's).

use bitvec::prelude::*;
use byteorder::{LittleEndian, ReadBytesExt};
use log::{debug, error, trace};
use std::{
    fs::File,
    io::{prelude::*, SeekFrom},
};
use thiserror::Error;
use usbsas_comm::{ComRpFs2Dev, ProtoRespCommon, ProtoRespFs2Dev, SendRecv};
use usbsas_proto as proto;
use usbsas_utils::SECTOR_SIZE;
#[cfg(not(feature = "mock"))]
use {
    std::os::unix::io::AsRawFd,
    usbsas_comm::ToFromFd,
    usbsas_mass_storage::{self, MassStorage},
};
#[cfg(feature = "mock")]
use {
    std::{env, fs::OpenOptions},
    usbsas_mock::mass_storage::MockMassStorage as MassStorage,
};

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("mass_storage error: {0}")]
    MassStorage(#[from] usbsas_mass_storage::Error),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("var error: {0}")]
    VarError(#[from] std::env::VarError),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
pub type Result<T> = std::result::Result<T, Error>;

// Some usb keys don't support bigger buffers
// (Linux writes 240 sectors per scsi write(10) requests)
const MAX_WRITE_SECTORS: usize = 240;
const BUFFER_MAX_WRITE_SIZE: u64 = MAX_WRITE_SECTORS as u64 * SECTOR_SIZE;

enum State {
    Init(InitState),
    DevOpened(DevOpenedState),
    BitVecLoaded(BitVecLoadedState),
    Copying(CopyingState),
    Wiping(WipingState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpFs2Dev) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::DevOpened(s) => s.run(comm),
            State::BitVecLoaded(s) => s.run(comm),
            State::WaitEnd(s) => s.run(comm),
            State::Copying(s) => s.run(comm),
            State::Wiping(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {
    fs_fname: String,
}

struct DevOpenedState {
    fs: File,
    mass_storage: MassStorage,
}

struct BitVecLoadedState {
    fs: File,
    fs_bv: BitVecIterOnes,
    mass_storage: MassStorage,
}

struct CopyingState {
    fs: File,
    fs_bv: BitVecIterOnes,
    mass_storage: MassStorage,
}

struct WipingState {
    fs: File,
    mass_storage: MassStorage,
}

struct WaitEndState;

// Wrapper around BitVec to iterate over contiguous group of ones
struct BitVecIterOnes {
    pub bv: BitVec<u8, Lsb0>,
    pos: usize,
    next_stop: usize,
}

impl BitVecIterOnes {
    fn new(bv: BitVec<u8, Lsb0>) -> Self {
        BitVecIterOnes {
            bv,
            pos: 0,
            next_stop: 0,
        }
    }
    fn count_ones(&self) -> usize {
        self.bv.count_ones()
    }
}

impl Iterator for BitVecIterOnes {
    type Item = (u64, u64);

    fn next(&mut self) -> Option<Self::Item> {
        let index_start = self.pos + self.bv[self.pos..].iter().position(|bit| *bit)?;
        if self.next_stop <= index_start {
            self.next_stop = index_start
                + self.bv[index_start..]
                    .iter()
                    .position(|bit| !*bit)
                    .unwrap_or_else(|| self.bv[index_start..].len());
        }
        self.pos = if self.next_stop - index_start > MAX_WRITE_SECTORS {
            index_start + MAX_WRITE_SECTORS
        } else {
            self.next_stop
        };
        Some((index_start as u64, self.pos as u64))
    }
}

impl InitState {
    fn run(self, comm: &mut ComRpFs2Dev) -> Result<State> {
        let busnum = comm.read_u32::<LittleEndian>()?;
        let devnum = comm.read_u32::<LittleEndian>()?;

        debug!("unlocked with busnum={} devnum={}", busnum, devnum);

        if busnum == 0 && devnum == 0 {
            #[cfg(not(feature = "mock"))]
            usbsas_sandbox::fs2dev::seccomp(comm.input_fd(), comm.output_fd(), None, None)?;
            return Ok(State::WaitEnd(WaitEndState));
        }

        let fs = File::open(self.fs_fname)?;

        #[cfg(not(feature = "mock"))]
        let (device_file, device_fd) = {
            let device_path = format!("/dev/bus/usb/{:03}/{:03}", busnum, devnum);
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(device_path)
            {
                Ok(file) => {
                    let file_fd = file.as_raw_fd();
                    (file, file_fd)
                }
                Err(err) => {
                    error!("Error opening device file: {}", err);
                    comm.error(err)?;
                    return Ok(State::WaitEnd(WaitEndState {}));
                }
            }
        };

        #[cfg(feature = "mock")]
        let device_file = OpenOptions::new()
            .read(false)
            .write(true)
            .open(env::var("USBSAS_MOCK_OUT_DEV")?)?;

        let mass_storage = MassStorage::from_opened_file(device_file)?;

        #[cfg(not(feature = "mock"))]
        usbsas_sandbox::fs2dev::seccomp(
            comm.input_fd(),
            comm.output_fd(),
            Some(fs.as_raw_fd()),
            Some(device_fd),
        )?;

        Ok(State::DevOpened(DevOpenedState { fs, mass_storage }))
    }
}

impl CopyingState {
    fn run(mut self, comm: &mut ComRpFs2Dev) -> Result<State> {
        trace!("copying state");
        comm.startcopy(proto::fs2dev::ResponseStartCopy {})?;

        let fs_size = self.fs.seek(SeekFrom::End(0))?;
        self.fs.rewind()?;

        let total_size = self.fs_bv.count_ones() as u64 * SECTOR_SIZE;

        trace!("state=copying: size={} ", total_size);

        let mut current_size = 0u64;
        let mut buffer = vec![0; BUFFER_MAX_WRITE_SIZE as usize];

        for (sector_start, sector_stop) in self.fs_bv {
            let sector_start_pos = sector_start * SECTOR_SIZE;
            self.fs.seek(SeekFrom::Start(sector_start_pos))?;

            let sector_count = sector_stop - sector_start;
            let sector_write_size = sector_count * SECTOR_SIZE;

            let (size, pad) = if sector_start_pos + sector_write_size > fs_size {
                let size = fs_size - sector_start_pos;
                (size, (sector_write_size - size))
            } else {
                (sector_write_size, 0)
            };

            self.fs.read_exact(&mut buffer[..size as usize])?;
            buffer[size as usize..]
                .iter_mut()
                .take(pad as usize)
                .for_each(|b| *b = 0);

            self.mass_storage.scsi_write_10(
                &mut buffer[..size as usize + pad as usize],
                sector_start,
                sector_count,
            )?;

            current_size += sector_write_size;
            comm.status(current_size, total_size, false)?;
        }
        comm.status(current_size, total_size, true)?;
        Ok(State::WaitEnd(WaitEndState))
    }
}

impl WipingState {
    fn run(mut self, comm: &mut ComRpFs2Dev) -> Result<State> {
        trace!("wiping state");
        comm.wipe(proto::fs2dev::ResponseWipe {})?;
        let mut buffer = vec![0u8; BUFFER_MAX_WRITE_SIZE as usize];
        let total_size = self.mass_storage.dev_size;
        let mut todo = total_size;
        let mut sector_index = 0;
        let mut sector_count = buffer.len() as u64 / SECTOR_SIZE;
        let mut current_size = 0;
        trace!(
            "wipe device; size: {} total sectors: {}",
            total_size,
            total_size / SECTOR_SIZE
        );

        while todo > 0 {
            trace!(
                "wipe cur size: {}, sector index: {}, todo: {}",
                current_size,
                sector_index,
                todo
            );
            if todo < buffer.len() as u64 {
                sector_count = todo / SECTOR_SIZE;
                buffer.truncate(todo as usize);
            }
            self.mass_storage
                .scsi_write_10(&mut buffer, sector_index, sector_count)?;
            current_size += buffer.len() as u64;
            comm.status(current_size, total_size, false)?;

            todo -= buffer.len() as u64;
            sector_index += sector_count;
        }
        comm.status(current_size, total_size, true)?;
        Ok(State::DevOpened(DevOpenedState {
            fs: self.fs,
            mass_storage: self.mass_storage,
        }))
    }
}

impl DevOpenedState {
    fn run(self, comm: &mut ComRpFs2Dev) -> Result<State> {
        trace!("dev opened state");
        use proto::fs2dev;
        use proto::fs2dev::request::Msg;

        let req: fs2dev::Request = comm.recv()?;
        Ok(match req.msg.ok_or(Error::BadRequest)? {
            Msg::DevSize(_) => {
                comm.devsize(fs2dev::ResponseDevSize {
                    size: self.mass_storage.dev_size,
                })?;
                State::DevOpened(self)
            }
            Msg::LoadBitVec(ref mut msg) => self.load_bitvec(comm, &mut msg.chunk, msg.last)?,
            Msg::Wipe(_) => State::Wiping(WipingState {
                fs: self.fs,
                mass_storage: self.mass_storage,
            }),
            Msg::End(_) => {
                comm.end()?;
                State::End
            }
            _ => {
                error!("bad request");
                comm.error("bad request")?;
                return Err(Error::State);
            }
        })
    }

    fn load_bitvec(self, comm: &mut ComRpFs2Dev, chunk: &mut Vec<u8>, last: bool) -> Result<State> {
        use proto::fs2dev::{self, request::Msg};
        let mut fs_bv_buf = Vec::new();
        fs_bv_buf.append(chunk);
        comm.loadbitvec(fs2dev::ResponseLoadBitVec {})?;
        if !last {
            loop {
                let req: fs2dev::Request = comm.recv()?;
                match req.msg.ok_or(Error::BadRequest)? {
                    Msg::LoadBitVec(ref mut msg) => {
                        fs_bv_buf.append(&mut msg.chunk);
                        comm.loadbitvec(fs2dev::ResponseLoadBitVec {})?;
                        if msg.last {
                            break;
                        }
                    }
                    _ => {
                        error!("bad request");
                        comm.error("bad request")?;
                        return Err(Error::State);
                    }
                }
            }
        }
        let fs_bv = BitVecIterOnes::new(BitVec::from_vec(fs_bv_buf));
        Ok(State::BitVecLoaded(BitVecLoadedState {
            fs: self.fs,
            fs_bv,
            mass_storage: self.mass_storage,
        }))
    }
}

impl BitVecLoadedState {
    fn run(self, comm: &mut ComRpFs2Dev) -> Result<State> {
        trace!("bitvec loaded state");
        use proto::fs2dev::{self, request::Msg};
        let req: fs2dev::Request = comm.recv()?;
        Ok(match req.msg.ok_or(Error::BadRequest)? {
            Msg::StartCopy(_) => State::Copying(CopyingState {
                fs: self.fs,
                fs_bv: self.fs_bv,
                mass_storage: self.mass_storage,
            }),
            Msg::End(_) => {
                comm.end()?;
                State::End
            }
            _ => {
                error!("bad request");
                comm.error("bad request")?;
                return Err(Error::State);
            }
        })
    }
}

impl WaitEndState {
    fn run(self, comm: &mut ComRpFs2Dev) -> Result<State> {
        use proto::fs2dev;
        use proto::fs2dev::request::Msg;

        let req: fs2dev::Request = comm.recv()?;

        match req.msg.ok_or(Error::BadRequest)? {
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

pub struct Fs2Dev {
    comm: ComRpFs2Dev,
    state: State,
}

impl Fs2Dev {
    pub fn new(comm: ComRpFs2Dev, fs_fname: String) -> Result<Self> {
        let state = State::Init(InitState { fs_fname });
        Ok(Fs2Dev { comm, state })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}, waiting end", err);
                    comm.error("bad request")?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}
