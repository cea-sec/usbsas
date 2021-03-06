//! usbsas process responsible for writing the file system on destination USB
//! device. It can also wipe devices (with 0's).

use bitvec::prelude::*;
use byteorder::{LittleEndian, ReadBytesExt};
use log::{debug, error, trace};
#[cfg(not(feature = "mock"))]
use rusb::{Context, UsbContext};
use std::{
    fs::File,
    io::{prelude::*, SeekFrom},
    os::unix::io::RawFd,
};
use thiserror::Error;
use usbsas_comm::{protoresponse, Comm};
#[cfg(feature = "mock")]
use usbsas_mock::mass_storage::{
    MockContext, MockMassStorage as MassStorage, MockUsbContext as UsbContext,
};
use usbsas_process::UsbsasProcess;
use usbsas_proto as proto;
use usbsas_utils::SECTOR_SIZE;
#[cfg(not(feature = "mock"))]
use {
    std::os::unix::io::AsRawFd,
    usbsas_mass_storage::{self, MassStorage},
};

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("rusb error: {0}")]
    Rusb(#[from] rusb::Error),
    #[error("privileges: {0}")]
    Privileges(#[from] usbsas_privileges::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
type Result<T> = std::result::Result<T, Error>;

protoresponse!(
    CommFs2Dev,
    fs2dev,
    end = End[ResponseEnd],
    error = Error[ResponseError],
    devsize = DevSize[ResponseDevSize],
    startcopy = StartCopy[ResponseStartCopy],
    copystatus = CopyStatus[ResponseCopyStatus],
    copystatusdone = CopyStatusDone[ResponseCopyStatusDone],
    loadbitvec = LoadBitVec[ResponseLoadBitVec],
    wipe = Wipe[ResponseWipe]
);

// Some usb keys don't support bigger buffers
// (Linux writes 240 sectors per scsi write(10) requests)
const MAX_WRITE_SECTORS: usize = 240;
const BUFFER_MAX_WRITE_SIZE: u64 = MAX_WRITE_SECTORS as u64 * SECTOR_SIZE;

enum State<T: UsbContext> {
    Init(InitState<T>),
    DevOpened(DevOpenedState<T>),
    BitVecLoaded(BitVecLoadedState<T>),
    Copying(CopyingState<T>),
    Wiping(WipingState<T>),
    WaitEnd(WaitEndState),
    End,
}

impl<T: UsbContext> State<T> {
    fn run(self, comm: &mut Comm<proto::fs2dev::Request>) -> Result<Self> {
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

struct InitState<T: UsbContext> {
    fs_fname: String,
    context: T,
}

struct DevOpenedState<T: UsbContext> {
    fs: File,
    mass_storage: MassStorage<T>,
}

struct BitVecLoadedState<T: UsbContext> {
    fs: File,
    fs_bv: BitVecIterOnes,
    mass_storage: MassStorage<T>,
}

struct CopyingState<T: UsbContext> {
    fs: File,
    fs_bv: BitVecIterOnes,
    mass_storage: MassStorage<T>,
}

struct WipingState<T: UsbContext> {
    fs: File,
    mass_storage: MassStorage<T>,
}

struct WaitEndState;

// Wrapper around BitVec to iterate over contiguous group of ones
struct BitVecIterOnes {
    pub bv: BitVec<u8, Lsb0>,
    pos: usize,
}

impl BitVecIterOnes {
    fn new(bv: BitVec<u8, Lsb0>) -> Self {
        BitVecIterOnes { bv, pos: 0 }
    }
    fn count_ones(&self) -> usize {
        self.bv.count_ones()
    }
}

impl Iterator for BitVecIterOnes {
    type Item = (u64, u64);

    fn next(&mut self) -> Option<Self::Item> {
        let index_start = self.pos + self.bv[self.pos..].iter().position(|bit| *bit)?;
        let index_stop = index_start
            + self.bv[index_start..]
                .iter()
                .position(|bit| !*bit)
                .unwrap_or_else(|| self.bv[index_start..].len());
        self.pos = if index_stop - index_start > MAX_WRITE_SECTORS {
            index_start + MAX_WRITE_SECTORS
        } else {
            index_stop
        };
        Some((index_start as u64, self.pos as u64))
    }
}

impl<T: UsbContext> InitState<T> {
    fn run(self, comm: &mut Comm<proto::fs2dev::Request>) -> Result<State<T>> {
        debug!("waiting for busnum and devnum");
        let busnum = comm.read_u32::<LittleEndian>()?;
        let devnum = comm.read_u32::<LittleEndian>()?;
        debug!("received busnum={} devnum={}", busnum, devnum);

        if busnum == 0 && devnum == 0 {
            #[cfg(not(feature = "mock"))]
            usbsas_privileges::fs2dev::drop_priv(
                comm.input_fd(),
                comm.output_fd(),
                None,
                usbsas_privileges::get_libusb_opened_fds(busnum, devnum)?,
            )?;
            Ok(State::WaitEnd(WaitEndState))
        } else {
            let fs = File::open(self.fs_fname)?;
            let mass_storage = MassStorage::from_busnum_devnum(self.context, busnum, devnum)?;
            #[cfg(not(feature = "mock"))]
            usbsas_privileges::fs2dev::drop_priv(
                comm.input_fd(),
                comm.output_fd(),
                Some(fs.as_raw_fd()),
                usbsas_privileges::get_libusb_opened_fds(busnum, devnum)?,
            )?;
            Ok(State::DevOpened(DevOpenedState { fs, mass_storage }))
        }
    }
}

impl<T: UsbContext> CopyingState<T> {
    fn run(mut self, comm: &mut Comm<proto::fs2dev::Request>) -> Result<State<T>> {
        trace!("copying state");
        comm.startcopy(proto::fs2dev::ResponseStartCopy {})?;

        let fs_size = self.fs.seek(SeekFrom::End(0))?;
        self.fs.seek(SeekFrom::Start(0))?;

        let total_size = self.fs_bv.count_ones() as u64 * SECTOR_SIZE;

        trace!("state=copying: size={} ", total_size);

        let mut current_size = 0u64;
        let mut buffer = vec![0; BUFFER_MAX_WRITE_SIZE as usize];

        for (sector_start, sector_stop) in self.fs_bv {
            let sector_start_pos = (sector_start * SECTOR_SIZE) as u64;
            self.fs.seek(SeekFrom::Start(sector_start_pos))?;

            let sector_count = sector_stop - sector_start;
            let sector_write_size = (sector_count * SECTOR_SIZE) as u64;

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
                sector_start as u64,
                sector_count as u64,
            )?;

            current_size += sector_write_size;
            comm.copystatus(proto::fs2dev::ResponseCopyStatus {
                current_size,
                total_size,
            })?;
        }

        comm.copystatusdone(proto::fs2dev::ResponseCopyStatusDone {})?;
        Ok(State::WaitEnd(WaitEndState))
    }
}

impl<T: UsbContext> WipingState<T> {
    fn run(mut self, comm: &mut Comm<proto::fs2dev::Request>) -> Result<State<T>> {
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
            total_size / SECTOR_SIZE as u64
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
            self.mass_storage.scsi_write_10(
                &mut buffer,
                sector_index as u64,
                sector_count as u64,
            )?;
            current_size += buffer.len() as u64;
            comm.copystatus(proto::fs2dev::ResponseCopyStatus {
                current_size,
                total_size,
            })?;

            todo -= buffer.len() as u64;
            sector_index += sector_count;
        }
        comm.copystatusdone(proto::fs2dev::ResponseCopyStatusDone {})?;
        Ok(State::DevOpened(DevOpenedState {
            fs: self.fs,
            mass_storage: self.mass_storage,
        }))
    }
}

impl<T: UsbContext> DevOpenedState<T> {
    fn run(self, comm: &mut Comm<proto::fs2dev::Request>) -> Result<State<T>> {
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
                comm.end(fs2dev::ResponseEnd {})?;
                State::End
            }
            _ => {
                error!("bad request");
                comm.error(fs2dev::ResponseError {
                    err: "bad request".into(),
                })?;
                return Err(Error::State);
            }
        })
    }

    fn load_bitvec(
        self,
        comm: &mut Comm<proto::fs2dev::Request>,
        chunk: &mut Vec<u8>,
        last: bool,
    ) -> Result<State<T>> {
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
                        comm.error(fs2dev::ResponseError {
                            err: "bad request".into(),
                        })?;
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

impl<T: UsbContext> BitVecLoadedState<T> {
    fn run(self, comm: &mut Comm<proto::fs2dev::Request>) -> Result<State<T>> {
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
                comm.end(fs2dev::ResponseEnd {})?;
                State::End
            }
            _ => {
                error!("bad request");
                comm.error(fs2dev::ResponseError {
                    err: "bad request".into(),
                })?;
                return Err(Error::State);
            }
        })
    }
}

impl WaitEndState {
    fn run<T: UsbContext>(self, comm: &mut Comm<proto::fs2dev::Request>) -> Result<State<T>> {
        use proto::fs2dev;
        use proto::fs2dev::request::Msg;

        let req: fs2dev::Request = comm.recv()?;

        match req.msg.ok_or(Error::BadRequest)? {
            Msg::End(_) => {
                comm.end(fs2dev::ResponseEnd {})?;
            }
            _ => {
                error!("bad request");
                comm.error(fs2dev::ResponseError {
                    err: "bad request".into(),
                })?;
            }
        }
        Ok(State::End)
    }
}

struct Fs2DevContext<T: UsbContext> {
    comm: Comm<proto::fs2dev::Request>,
    state: State<T>,
}

impl<T: UsbContext> Fs2DevContext<T> {
    fn new(comm: Comm<proto::fs2dev::Request>, fs_fname: String, context: T) -> Result<Self> {
        let state = State::Init(InitState { fs_fname, context });
        Ok(Fs2DevContext { comm, state })
    }

    fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}, waiting end", err);
                    comm.error(proto::fs2dev::ResponseError {
                        err: format!("run error: {}", err),
                    })?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}

// Wrapper to avoid impl UsbsasProcess for Fs2Dev<T>
pub struct Fs2Dev {}

impl UsbsasProcess for Fs2Dev {
    fn spawn(
        read_fd: RawFd,
        write_fd: RawFd,
        args: Option<Vec<String>>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        #[cfg(not(feature = "mock"))]
        assert!(rusb::supports_detach_kernel_driver());

        if let Some(args) = args {
            if let Some(fname) = args.get(0) {
                debug!("fs2dev: {}", fname);
                Fs2DevContext::new(
                    Comm::from_raw_fd(read_fd, write_fd),
                    fname.to_owned(),
                    #[cfg(not(feature = "mock"))]
                    Context::new()?,
                    #[cfg(feature = "mock")]
                    MockContext {},
                )?
                .main_loop()
                .map(|_| debug!("fs2dev: exit"))?;
                return Ok(());
            }
        }
        Err(Box::new(Error::Error(
            "fs2dev needs a fname arg".to_string(),
        )))
    }
}
