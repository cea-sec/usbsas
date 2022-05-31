//! `usbsas`'s `tar2files` process. It is responsible for reading files in the
//! temp tar archive. It answers to usbsas's `readdir`, `getattr`, `readfile`,
//! etc. requests

use byteorder::ReadBytesExt;
use log::{error, info, trace};
use std::{
    collections::HashMap,
    convert::TryFrom,
    fs::File,
    io::{prelude::*, SeekFrom},
    os::unix::io::{AsRawFd, RawFd},
};
use tar::Archive;
use thiserror::Error;
use usbsas_comm::{protoresponse, Comm};
use usbsas_process::UsbsasProcess;
use usbsas_proto as proto;
use usbsas_proto::{
    common::{FileInfo, FileType},
    files::request::Msg,
};
use usbsas_utils::READ_FILE_MAX_SIZE;

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("privileges: {0}")]
    Privileges(#[from] usbsas_privileges::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
type Result<T> = std::result::Result<T, Error>;

protoresponse!(
    CommReadTar,
    files,
    getattr = GetAttr[ResponseGetAttr],
    readdir = ReadDir[ResponseReadDir],
    readfile = ReadFile[ResponseReadFile],
    error = Error[ResponseError],
    end = End[ResponseEnd]
);

enum State {
    Init(InitState),
    LoadMetadata(LoadMetadataState),
    MainLoop(MainLoopState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut Comm<proto::files::Request>) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::LoadMetadata(s) => s.run(comm),
            State::MainLoop(s) => s.run(comm),
            State::WaitEnd(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

#[derive(Debug)]
struct Attrs {
    ftype: FileType,
    size: u64,
    timestamp: i64,
    offset: u64,
}

struct InitState {
    tarpath: String,
}

struct LoadMetadataState {
    tar: File,
}

struct MainLoopState {
    metadata: HashMap<String, Attrs>,
    archive: File,
}

struct WaitEndState;

impl InitState {
    fn run(self, comm: &mut Comm<proto::files::Request>) -> Result<State> {
        trace!("waiting unlock");
        Ok(match comm.read_u8()? {
            1 => {
                let tar = File::open(self.tarpath)?;
                usbsas_privileges::tar2files::drop_priv(
                    comm.input_fd(),
                    comm.output_fd(),
                    Some(tar.as_raw_fd()),
                )?;
                State::LoadMetadata(LoadMetadataState { tar })
            }
            _ => {
                usbsas_privileges::tar2files::drop_priv(comm.input_fd(), comm.output_fd(), None)?;
                State::WaitEnd(WaitEndState {})
            }
        })
    }
}

impl LoadMetadataState {
    fn run(self, _comm: &mut Comm<proto::files::Request>) -> Result<State> {
        let mut metadata = HashMap::new();
        let mut archive = Archive::new(self.tar);

        // Read tar headers once
        for entry in archive.entries()? {
            let entry = entry?;
            let ftype = match entry.header().entry_type() {
                tar::EntryType::Directory => FileType::Directory,
                tar::EntryType::Regular => FileType::Regular,
                _ => continue,
            };
            metadata.insert(
                entry.path()?.to_path_buf().to_string_lossy().to_string(),
                Attrs {
                    ftype,
                    size: entry.header().size()?,
                    timestamp: i64::try_from(entry.header().mtime()?)?,
                    offset: entry.raw_file_position(),
                },
            );
        }

        Ok(State::MainLoop(MainLoopState {
            metadata,
            archive: archive.into_inner(),
        }))
    }
}

impl MainLoopState {
    fn run(mut self, comm: &mut Comm<proto::files::Request>) -> Result<State> {
        trace!("main loop");
        loop {
            let req: proto::files::Request = comm.recv()?;

            let res = match req.msg.ok_or(Error::BadRequest)? {
                Msg::GetAttr(req) => self.getattr(comm, &req.path),
                Msg::ReadFile(req) => self.readfile(comm, &req.path, req.offset, req.size as usize),
                Msg::ReadDir(req) => self.readdir(comm, &req.path),
                Msg::End(_) => {
                    comm.end(proto::files::ResponseEnd {})?;
                    return Ok(State::End);
                }
                _ => {
                    error!("unexpected req");
                    Err(Error::BadRequest)
                }
            };

            match res {
                Ok(_) => continue,
                Err(err) => {
                    error!("{}", err);
                    comm.error(proto::files::ResponseError {
                        err: format!("{}", err),
                    })?;
                }
            }
        }
    }

    fn get_entry(&self, path: &str) -> Result<&Attrs> {
        let path = path.trim_start_matches('/');
        self.metadata
            .get(path)
            .ok_or_else(|| Error::Error(format!("didn't find {} in metadata", path)))
    }

    fn getattr(&mut self, comm: &mut Comm<proto::files::Request>, path: &str) -> Result<()> {
        trace!("req_getattr: {}", path);
        let entry = self.get_entry(path)?;
        Ok(comm.getattr(proto::files::ResponseGetAttr {
            ftype: entry.ftype.into(),
            size: entry.size,
            timestamp: entry.timestamp,
        })?)
    }

    fn readfile(
        &mut self,
        comm: &mut Comm<proto::files::Request>,
        path: &str,
        file_offset: u64,
        size: usize,
    ) -> Result<()> {
        trace!("req_readfile {}", path);

        if size as u64 > READ_FILE_MAX_SIZE {
            return Err(Error::Error("max read size exceded".to_string()));
        }

        let mut data = vec![0u8; size];

        let entry_offset = self.get_entry(path)?.offset;

        self.archive
            .seek(SeekFrom::Start(entry_offset + file_offset))?;
        self.archive.read_exact(&mut data)?;
        Ok(comm.readfile(proto::files::ResponseReadFile { data })?)
    }

    fn readdir(&mut self, comm: &mut Comm<proto::files::Request>, path: &str) -> Result<()> {
        info!("req read_dir {}", path);
        let path = path.trim_start_matches('/');
        let filesinfo = self
            .metadata
            .iter()
            .filter(|(entry, _)| {
                entry.starts_with(path)
                    && !entry
                        .trim_start_matches(path)
                        .trim_start_matches('/')
                        .contains('/')
                    && entry != &path
            })
            .map(|(entry, attrs)| FileInfo {
                path: entry.clone(),
                ftype: attrs.ftype.into(),
                size: attrs.size,
                timestamp: attrs.timestamp,
            })
            .collect::<Vec<FileInfo>>();

        Ok(comm.readdir(proto::files::ResponseReadDir { filesinfo })?)
    }
}

impl WaitEndState {
    fn run(self, comm: &mut Comm<proto::files::Request>) -> Result<State> {
        trace!("wait end state");
        let req: proto::files::Request = comm.recv()?;
        match req.msg.ok_or(Error::BadRequest)? {
            Msg::End(_) => {
                comm.end(proto::files::ResponseEnd {})?;
            }
            _ => {
                error!("unexpected req");
                comm.error(proto::files::ResponseError {
                    err: "bad request".into(),
                })?;
            }
        }
        Ok(State::End)
    }
}

pub struct Tar2Files {
    comm: Comm<proto::files::Request>,
    state: State,
}

impl Tar2Files {
    fn new(comm: Comm<proto::files::Request>, tarpath: String) -> Result<Self> {
        let state = State::Init(InitState { tarpath });
        Ok(Tar2Files { comm, state })
    }

    fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}", err);
                    comm.error(proto::files::ResponseError {
                        err: format!("run error: {}", err),
                    })?;
                    State::WaitEnd(WaitEndState {})
                }
            }
        }
        Ok(())
    }
}

impl UsbsasProcess for Tar2Files {
    fn spawn(
        read_fd: RawFd,
        write_fd: RawFd,
        args: Option<Vec<String>>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        if let Some(args) = args {
            if let Some(fname) = args.get(0) {
                log::info!("tar2files: {}", fname);
                Tar2Files::new(Comm::from_raw_fd(read_fd, write_fd), fname.to_owned())?
                    .main_loop()
                    .map(|_| log::debug!("tar2files exit"))?;
                return Ok(());
            }
        }
        Err(Box::new(Error::Error(
            "tar2files needs a tar fname arg".to_string(),
        )))
    }
}
