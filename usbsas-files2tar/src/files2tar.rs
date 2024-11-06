use crate::{tarwriter::TarWriter, ArchiveWriter};
use crate::{Error, Result};
use log::{error, trace};
use std::{fs, os::unix::io::AsRawFd};
use usbsas_comm::{ComRpWriteTar, ProtoRespCommon, ProtoRespWriteTar, ToFromFd};
use usbsas_proto as proto;
use usbsas_proto::{common::FileType, writetar::request::Msg};

enum State {
    Init(InitState),
    WaitNewFile(WaitNewFileState),
    WritingFile(WritingFileState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpWriteTar) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::WaitNewFile(s) => s.run(comm),
            State::WritingFile(s) => s.run(comm),
            State::WaitEnd(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {
    archive_path: String,
}

impl InitState {
    fn run(self, comm: &mut ComRpWriteTar) -> Result<State> {
        let archive_file = fs::OpenOptions::new()
            .read(false)
            .write(true)
            .open(self.archive_path)?;
        let outfd = archive_file.as_raw_fd();

        let mut archive: Box<dyn ArchiveWriter> = Box::new(TarWriter::new(archive_file));

        usbsas_sandbox::files2tar::seccomp(comm.input_fd(), comm.output_fd(), outfd)?;

        archive.init()?;
        Ok(State::WaitNewFile(WaitNewFileState { archive }))
    }
}

struct WaitNewFileState {
    archive: Box<dyn ArchiveWriter>,
}

impl WaitNewFileState {
    fn run(mut self, comm: &mut ComRpWriteTar) -> Result<State> {
        match comm.recv_req()? {
            Msg::NewFile(req) => {
                let fstype =
                    FileType::try_from(req.ftype).map_err(|err| Error::Error(format!("{err}")))?;
                match self
                    .archive
                    .newfile(&req.path, fstype, req.size, req.timestamp)
                {
                    Ok(_) => {
                        comm.newfile(proto::writetar::ResponseNewFile {})?;
                        Ok(State::WritingFile(WritingFileState {
                            archive: self.archive,
                            total_size: req.size as usize,
                            len_written: 0,
                        }))
                    }
                    Err(err) => {
                        error!("Couldn't add file \"{}\": {}", &req.path, err);
                        comm.error(err)?;
                        Ok(State::WaitNewFile(self))
                    }
                }
            }
            Msg::Close(req) => {
                self.archive.finish(&req.infos)?;
                comm.close(proto::writetar::ResponseClose {})?;
                Ok(State::WaitEnd(WaitEndState {}))
            }
            Msg::End(_) => {
                comm.end()?;
                Ok(State::End)
            }
            _ => {
                error!("unexpected req");
                Err(Error::BadRequest)
            }
        }
    }
}

struct WritingFileState {
    archive: Box<dyn ArchiveWriter>,
    len_written: usize,
    total_size: usize,
}

impl WritingFileState {
    fn run(mut self, comm: &mut ComRpWriteTar) -> Result<State> {
        loop {
            match comm.recv_req()? {
                Msg::WriteFile(req) => {
                    self.len_written += req.data.len();
                    if self.len_written > self.total_size {
                        return Err(Error::Error(
                            "Data oversize while writing file in archive".to_string(),
                        ));
                    }
                    if let Err(err) = self.archive.writefile(&req.data) {
                        return Err(Error::Error(format!("{err}")));
                    } else {
                        comm.writefile(proto::writetar::ResponseWriteFile {})?;
                    }
                }
                Msg::EndFile(_) => {
                    if let Err(err) = self.archive.endfile(self.len_written) {
                        return Err(Error::Error(format!("{err}")));
                    };
                    comm.endfile(proto::writetar::ResponseEndFile {})?;
                    return Ok(State::WaitNewFile(WaitNewFileState {
                        archive: self.archive,
                    }));
                }
                _ => {
                    error!("unexpected req");
                    return Err(Error::BadRequest);
                }
            }
        }
    }
}

struct WaitEndState {}

impl WaitEndState {
    fn run(self, comm: &mut ComRpWriteTar) -> Result<State> {
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

pub struct Files2Tar {
    comm: ComRpWriteTar,
    state: State,
}

impl Files2Tar {
    pub fn new(comm: ComRpWriteTar, archive_path: String) -> Result<Self> {
        let state = State::Init(InitState { archive_path });
        Ok(Files2Tar { comm, state })
    }
    pub fn new_end(comm: ComRpWriteTar) -> Result<Self> {
        let state = State::WaitEnd(WaitEndState {});
        Ok(Files2Tar { comm, state })
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
            }
        }
        Ok(())
    }
}
