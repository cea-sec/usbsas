//! `usbsas`'s `tar2files` process. It is responsible for reading files in the
//! temp tar archive. It answers to usbsas's `readdir`, `getattr`, `readfile`,
//! etc. requests

use byteorder::{LittleEndian, ReadBytesExt};
use log::{debug, error, trace};
use std::{
    collections::HashMap,
    convert::TryFrom,
    fs::File,
    io::{prelude::*, SeekFrom},
    os::unix::io::AsRawFd,
};
use tar::Archive;
use thiserror::Error;
use usbsas_comm::{ComRpFiles, ProtoRespCommon, ProtoRespFiles, ToFd};
use usbsas_proto as proto;
use usbsas_proto::{
    common::{FileInfo, FileType},
    files::request::Msg,
};
use usbsas_utils::{READ_FILE_MAX_SIZE, TAR_DATA_DIR};

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
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
    LoadMetadata(LoadMetadataState),
    MainLoop(MainLoopState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpFiles) -> Result<Self> {
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
    fn run(self, comm: &mut ComRpFiles) -> Result<State> {
        trace!("waiting unlock");
        Ok(match comm.read_u64::<LittleEndian>()? {
            1 => {
                let tar = File::open(self.tarpath)?;
                usbsas_sandbox::tar2files::seccomp(
                    comm.input_fd(),
                    comm.output_fd(),
                    Some(tar.as_raw_fd()),
                )?;
                State::LoadMetadata(LoadMetadataState { tar })
            }
            _ => {
                usbsas_sandbox::tar2files::seccomp(comm.input_fd(), comm.output_fd(), None)?;
                State::WaitEnd(WaitEndState {})
            }
        })
    }
}

impl LoadMetadataState {
    fn run(self, _comm: &mut ComRpFiles) -> Result<State> {
        let mut metadata = HashMap::new();
        let mut archive = Archive::new(self.tar);
        let data_dir = TAR_DATA_DIR.trim_end_matches('/').to_owned() + "/";

        // Read tar headers once
        for entry in archive.entries()? {
            let entry = entry?;
            let path_name = entry.path()?.to_path_buf().to_string_lossy().to_string();
            let ftype = match entry.header().entry_type() {
                tar::EntryType::Directory => FileType::Directory,
                tar::EntryType::Regular => FileType::Regular,
                _ => continue,
            };
            if let Some(name) = path_name.strip_prefix(&data_dir) {
                metadata.insert(
                    name.trim_end_matches('/').to_owned(),
                    Attrs {
                        ftype,
                        size: entry.header().size()?,
                        timestamp: i64::try_from(entry.header().mtime()?)?,
                        offset: entry.raw_file_position(),
                    },
                );
            } else if path_name == "config.json" {
                metadata.insert(
                    path_name,
                    Attrs {
                        ftype,
                        size: entry.header().size()?,
                        timestamp: i64::try_from(entry.header().mtime()?)?,
                        offset: entry.raw_file_position(),
                    },
                );
            } else {
                log::debug!("file '{path_name}' not in '{data_dir}' dir, ignoring");
            }
        }

        Ok(State::MainLoop(MainLoopState {
            metadata,
            archive: archive.into_inner(),
        }))
    }
}

impl MainLoopState {
    fn run(mut self, comm: &mut ComRpFiles) -> Result<State> {
        trace!("main loop");
        loop {
            let res = match comm.recv_req()? {
                Msg::GetAttr(req) => self.getattr(comm, &req.path),
                Msg::ReadFile(req) => self.readfile(comm, &req.path, req.offset, req.size as usize),
                Msg::ReadDir(req) => self.readdir(comm, &req.path),
                Msg::End(_) => {
                    comm.end()?;
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
                    error!("{err}");
                    comm.error(err)?;
                }
            }
        }
    }

    fn get_entry(&self, path: &str) -> Result<&Attrs> {
        let path = path.trim_start_matches('/').trim_end_matches('/');
        self.metadata
            .get(path)
            .ok_or_else(|| Error::Error(format!("didn't find {path} in metadata")))
    }

    fn getattr(&mut self, comm: &mut ComRpFiles, path: &str) -> Result<()> {
        trace!("req_getattr: {path}");
        let entry = self.get_entry(path)?;
        Ok(comm.getattr(proto::files::ResponseGetAttr {
            ftype: entry.ftype.into(),
            size: entry.size,
            timestamp: entry.timestamp,
        })?)
    }

    fn readfile(
        &mut self,
        comm: &mut ComRpFiles,
        path: &str,
        file_offset: u64,
        size: usize,
    ) -> Result<()> {
        debug!("req_readfile {path}");

        if size as u64 > READ_FILE_MAX_SIZE {
            return Err(Error::Error("max read size exceeded".to_string()));
        }

        let mut data = vec![0u8; size];

        let entry_offset = self.get_entry(path)?.offset;

        self.archive
            .seek(SeekFrom::Start(entry_offset + file_offset))?;
        self.archive.read_exact(&mut data)?;
        Ok(comm.readfile(proto::files::ResponseReadFile { data })?)
    }

    fn readdir(&mut self, comm: &mut ComRpFiles, path: &str) -> Result<()> {
        debug!("req read_dir {path}");
        let path = path.trim_start_matches('/');
        let filesinfo = self
            .metadata
            .iter()
            .filter(|(entry, _)| {
                entry.starts_with(path)
                    && !entry
                        .strip_prefix(path)
                        .unwrap()
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
    fn run(self, comm: &mut ComRpFiles) -> Result<State> {
        trace!("wait end state");
        match comm.recv_req()? {
            Msg::End(_) => {
                comm.end()?;
            }
            _ => {
                error!("unexpected req");
                comm.error("bad request")?;
            }
        }
        Ok(State::End)
    }
}

pub struct Tar2Files {
    comm: ComRpFiles,
    state: State,
}

impl Tar2Files {
    pub fn new(comm: ComRpFiles, tarpath: String) -> Result<Self> {
        let state = State::Init(InitState { tarpath });
        Ok(Tar2Files { comm, state })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {err}");
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            }
        }
        Ok(())
    }
}
