use crate::{tarwriter::TarWriter, ArchiveWriter};
use crate::{Error, Result};
use byteorder::ReadBytesExt;
use log::{error, trace};
use std::{
    fs,
    os::unix::io::{AsRawFd, RawFd},
};
use usbsas_comm::{protoresponse, Comm};
use usbsas_process::UsbsasProcess;
use usbsas_proto as proto;
use usbsas_proto::{common::FileType, writetar::request::Msg};

protoresponse!(
    CommWritefs,
    writetar,
    newfile = NewFile[ResponseNewFile],
    writefile = WriteFile[ResponseWriteFile],
    endfile = EndFile[ResponseEndFile],
    close = Close[ResponseClose],
    error = Error[ResponseError],
    end = End[ResponseEnd]
);

enum State {
    Init(InitState),
    WaitNewFile(WaitNewFileState),
    WritingFile(WritingFileState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut Comm<proto::writetar::Request>) -> Result<Self> {
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
    with_infos: bool,
}

impl InitState {
    fn run(self, comm: &mut Comm<proto::writetar::Request>) -> Result<State> {
        let archive_file = fs::OpenOptions::new()
            .read(false)
            .write(true)
            .open(&self.archive_path)?;
        let outfd = archive_file.as_raw_fd();

        let mut archive: Box<dyn ArchiveWriter> = if self.with_infos {
            Box::new(TarWriter::new(archive_file, Some("data/".into())))
        } else {
            Box::new(TarWriter::new(archive_file, None))
        };

        usbsas_privileges::files2tar::drop_priv(comm.input_fd(), comm.output_fd(), outfd)?;

        archive.init()?;
        Ok(State::WaitNewFile(WaitNewFileState { archive }))
    }
}

struct WaitNewFileState {
    archive: Box<dyn ArchiveWriter>,
}

impl WaitNewFileState {
    fn run(mut self, comm: &mut Comm<proto::writetar::Request>) -> Result<State> {
        let req: proto::writetar::Request = comm.recv()?;
        match req.msg.ok_or(Error::BadRequest)? {
            Msg::NewFile(req) => {
                let fstype = FileType::from_i32(req.ftype)
                    .ok_or_else(|| Error::Error("Bad file type".to_string()))?;
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
                        comm.error(proto::writetar::ResponseError {
                            err: format!("{}", err),
                        })?;
                        Ok(State::WaitNewFile(self))
                    }
                }
            }
            Msg::Close(req) => {
                self.archive.finish(req)?;
                comm.close(proto::writetar::ResponseClose {})?;
                Ok(State::WaitEnd(WaitEndState {}))
            }
            Msg::End(_) => {
                comm.end(proto::writetar::ResponseEnd {})?;
                Ok(State::End)
            }
            _ => {
                error!("unexpexted req");
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
    fn run(mut self, comm: &mut Comm<proto::writetar::Request>) -> Result<State> {
        loop {
            let req: proto::writetar::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::WriteFile(req) => {
                    self.len_written += req.data.len();
                    if self.len_written > self.total_size {
                        return Err(Error::Error(
                            "Data oversize while writing file in archive".to_string(),
                        ));
                    }
                    if let Err(err) = self.archive.writefile(&req.data) {
                        return Err(Error::Error(format!("{}", err)));
                    } else {
                        comm.writefile(proto::writetar::ResponseWriteFile {})?;
                    }
                }
                Msg::EndFile(_) => {
                    if let Err(err) = self.archive.endfile(self.len_written) {
                        return Err(Error::Error(format!("{}", err)));
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
    fn run(self, comm: &mut Comm<proto::writetar::Request>) -> Result<State> {
        trace!("wait end state");
        let req: proto::writetar::Request = comm.recv()?;
        match req.msg.ok_or(Error::BadRequest)? {
            Msg::End(_) => {
                comm.end(proto::writetar::ResponseEnd {})?;
            }
            _ => {
                error!("unexpected req");
                comm.error(proto::writetar::ResponseError {
                    err: "bad request".into(),
                })?;
            }
        }
        Ok(State::End)
    }
}

pub struct Files2Tar {
    comm: Comm<proto::writetar::Request>,
    state: State,
}

impl Files2Tar {
    fn new(
        comm: Comm<proto::writetar::Request>,
        archive_path: String,
        with_infos: bool,
    ) -> Result<Self> {
        let state = State::Init(InitState {
            archive_path,
            with_infos,
        });
        Ok(Files2Tar { comm, state })
    }

    fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}", err);
                    comm.error(proto::writetar::ResponseError {
                        err: format!("run error: {}", err),
                    })?;
                    State::WaitEnd(WaitEndState {})
                }
            }
        }
        Ok(())
    }
}

impl UsbsasProcess for Files2Tar {
    fn spawn(
        read_fd: RawFd,
        write_fd: RawFd,
        args: Option<Vec<String>>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        if let Some(args) = args {
            if let Some(fname) = args.get(0) {
                let mut comm = Comm::from_raw_fd(read_fd, write_fd);
                match comm.read_u8()? {
                    // 0: create a "flat" tar for the remote analyzer
                    0 => Files2Tar::new(comm, fname.to_string(), false)?.main_loop()?,
                    // 1: create a tar in which files will be stored under the "/data" directory.
                    //    "/info.json" will also be added with informations about the transfer.
                    1 => Files2Tar::new(comm, fname.to_string(), true)?.main_loop()?,
                    // 2: unlock to exit value
                    2 => Files2Tar {
                        comm,
                        state: State::WaitEnd(WaitEndState {}),
                    }
                    .main_loop()?,
                    _ => {
                        return Err(Box::new(Error::Error("Bad unlock value".to_string())));
                    }
                }
                return Ok(());
            }
        }
        Err(Box::new(Error::Error(
            "files2tar needs a tarpath arg".to_string(),
        )))
    }
}
