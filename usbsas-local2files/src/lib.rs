use log::{error, trace};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
};
use thiserror::Error;
use usbsas_comm::{ComRpFiles, ProtoRespCommon, ProtoRespFiles, ToFd};
use usbsas_config::{conf_parse, conf_read};
use usbsas_proto::{
    self as proto,
    common::{FileInfo, FileType},
    files::request::Msg,
};

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
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
    Running(RunningState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpFiles) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::Running(s) => s.run(comm),
            State::WaitEnd(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {
    config_path: String,
}

impl InitState {
    fn run(self, comm: &mut ComRpFiles) -> Result<State> {
        let conf_data = conf_read(&self.config_path)?;

        usbsas_sandbox::local2files::seccomp(comm.input_fd(), comm.output_fd())?;

        let config = conf_parse(&conf_data)?;
        let src_local_dirs = if let Some(conf) = config.src_local_dirs {
            conf
        } else {
            usbsas_sandbox::landlock(None, None, None, None, None)?;
            log::debug!("No configuration for local dirs, parking");
            return Ok(State::WaitEnd(WaitEndState {}));
        };

        let local_dirs: Vec<&str> = src_local_dirs.iter().map(|dir| dir.path.as_str()).collect();
        usbsas_sandbox::landlock(Some(&local_dirs), None, None, None, None)?;

        match comm.recv_req()? {
            Msg::OpenLocalDir(req) => {
                if local_dirs.contains(&req.path.as_str()) {
                    comm.openlocaldir(proto::files::ResponseOpenLocalDir {})?;
                    Ok(State::Running(RunningState {
                        basename: req.path,
                        opened_file: None,
                    }))
                } else {
                    Err(Error::BadRequest)
                }
            }
            Msg::End(_) => {
                comm.end()?;
                Ok(State::End)
            }
            _ => Err(Error::BadRequest),
        }
    }
}

struct RunningState {
    basename: String,
    opened_file: Option<(String, File)>,
}

impl RunningState {
    fn run(mut self, comm: &mut ComRpFiles) -> Result<State> {
        loop {
            let res = match comm.recv_req()? {
                Msg::GetAttr(req) => self.getattr(comm, req.path),
                Msg::ReadDir(req) => self.readdir(comm, req.path),
                Msg::ReadFile(req) => self.readfile(comm, req.path, req.offset, req.size),
                Msg::End(_) => break,
                _ => Err(Error::BadRequest),
            };
            if let Err(err) = res {
                comm.error(err)?;
            };
        }
        Ok(State::End)
    }

    fn getattr(&self, comm: &mut ComRpFiles, path: String) -> Result<()> {
        let metadata = fs::metadata(format!("{}/{}", self.basename.trim_end_matches('/'), path))?;
        if let Ok(time) = metadata.modified()
            && (metadata.is_file() || metadata.is_dir())
        {
            let ts = time
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or(std::time::Duration::new(0, 0));
            comm.getattr(proto::files::ResponseGetAttr {
                ftype: FileType::from(metadata.file_type()).into(),
                size: metadata.len(),
                timestamp: ts.as_secs() as i64,
            })?;
            return Ok(());
        }
        Err(Error::Error("getattr".to_string()))
    }

    fn readdir(&self, comm: &mut ComRpFiles, path: String) -> Result<()> {
        let mut filesinfo = vec![];
        for entry in fs::read_dir(format!("{}/{}", self.basename.trim_end_matches('/'), path))? {
            if let Ok(entry) = entry
                && let Ok(metadata) = entry.metadata()
                && let Ok(time) = metadata.modified()
                && (metadata.is_file() || metadata.is_dir())
            {
                let ts = time
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or(std::time::Duration::new(0, 0));
                filesinfo.push(FileInfo {
                    path: format!(
                        "{}/{}",
                        path.trim_end_matches('/'),
                        entry.file_name().into_string().unwrap()
                    ),
                    ftype: FileType::from(metadata.file_type()).into(),
                    size: metadata.len(),
                    timestamp: ts.as_secs() as i64,
                });
            }
        }
        filesinfo.sort_by(|a, b| a.path.cmp(&b.path));
        comm.readdir(proto::files::ResponseReadDir { filesinfo })?;
        Ok(())
    }

    fn readfile(
        &mut self,
        comm: &mut ComRpFiles,
        path: String,
        offset: u64,
        size: u64,
    ) -> Result<()> {
        let mut file = if let Some((opened_path, file)) = self.opened_file.take()
            && path == opened_path
        {
            file
        } else {
            File::open(format!("{}/{}", self.basename.trim_end_matches('/'), path))?
        };

        file.seek(SeekFrom::Start(offset))?;
        let mut data: Vec<u8> = vec![0; size as usize];
        file.read_exact(&mut data)?;
        comm.readfile(proto::files::ResponseReadFile { data })?;
        self.opened_file = Some((path, file));
        Ok(())
    }
}

struct WaitEndState {}

impl WaitEndState {
    fn run(self, comm: &mut ComRpFiles) -> Result<State> {
        trace!("wait end state");
        loop {
            match comm.recv_req()? {
                Msg::End(_) => {
                    comm.end()?;
                    break;
                }
                _ => {
                    error!("bad request");
                    comm.error("bad request")?;
                }
            }
        }
        Ok(State::End)
    }
}

pub struct Local2Files {
    comm: ComRpFiles,
    state: State,
}

impl Local2Files {
    pub fn new(comm: ComRpFiles, config_path: String) -> Result<Self> {
        let state = State::Init(InitState { config_path });
        Ok(Local2Files { comm, state })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {err}, waiting end");
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            }
        }
        Ok(())
    }
}
