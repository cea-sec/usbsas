use crate::{Error, HttpClient, Result};
use log::{error, trace};
use reqwest::blocking::Body;
use std::{
    fs::File,
    io::{self, Read},
    os::unix::io::RawFd,
};
use usbsas_comm::{protoresponse, Comm};
use usbsas_process::UsbsasProcess;
use usbsas_proto as proto;
use usbsas_proto::uploader::request::Msg;

protoresponse!(
    CommUploader,
    uploader,
    upload = Upload[ResponseUpload],
    uploadstatus = UploadStatus[ResponseUploadStatus],
    end = End[ResponseEnd],
    error = Error[ResponseError]
);

struct FileReaderProgress {
    comm: Comm<proto::uploader::Request>,
    file: File,
    filesize: u64,
    offset: u64,
}

impl Read for FileReaderProgress {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let size_read = self.file.read(buf)?;
        self.offset += size_read as u64;
        // if we report progression with each read (of 8kb), the json status of
        // the server polled by the client will quickly become very large and
        // will cause errors. 1 in 10 is enough.
        if (self.offset / size_read as u64) % 10 == 0 || self.offset == self.filesize {
            self.comm
                .uploadstatus(proto::uploader::ResponseUploadStatus {
                    current_size: self.offset,
                    total_size: self.filesize,
                })?;
        }
        Ok(size_read)
    }
}

enum State {
    Init(InitState),
    Running(RunningState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut Comm<proto::uploader::Request>) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::Running(s) => s.run(comm),
            State::WaitEnd(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {
    tarpath: String,
}

struct RunningState {
    file: Option<File>,
}

struct WaitEndState {}

impl InitState {
    fn run(self, _comm: &mut Comm<proto::uploader::Request>) -> Result<State> {
        let file = File::open(&self.tarpath)?;

        Ok(State::Running(RunningState { file: Some(file) }))
    }
}

impl RunningState {
    fn run(mut self, comm: &mut Comm<proto::uploader::Request>) -> Result<State> {
        let req: proto::uploader::Request = comm.recv()?;
        match req.msg.ok_or(Error::BadRequest)? {
            Msg::Upload(req) => {
                if let Err(err) = self.upload(comm, req) {
                    error!("upload error: {}", err);
                    comm.error(proto::uploader::ResponseError {
                        err: format!("{}", err),
                    })?;
                };
                Ok(State::WaitEnd(WaitEndState {}))
            }
            Msg::End(_) => {
                comm.end(proto::uploader::ResponseEnd {})?;
                Ok(State::End)
            }
        }
    }

    fn upload(
        &mut self,
        comm: &mut Comm<proto::uploader::Request>,
        req: proto::uploader::RequestUpload,
    ) -> Result<()> {
        trace!("upload");
        let dstnet = req.dstnet.ok_or(Error::BadRequest)?;
        let url = format!("{}/{}", dstnet.url.trim_end_matches('/'), req.id);
        let mut http_client = HttpClient::new(
            #[cfg(feature = "authkrb")]
            if !dstnet.krb_service_name.is_empty() {
                Some(dstnet.krb_service_name)
            } else {
                None
            },
        )?;
        let file = self
            .file
            .take()
            .ok_or_else(|| Error::Error("no file to upload".to_string()))?;
        let filesize = file.metadata()?.len();

        let comm_progress = comm.try_clone()?;

        let filereaderprogress = FileReaderProgress {
            comm: comm_progress,
            file,
            filesize,
            offset: 0,
        };

        let body = Body::sized(filereaderprogress, filesize);

        let resp = http_client.post(&url, body)?;
        if !resp.status().is_success() {
            return Err(Error::Upload(format!(
                "Unknown status code {:?}",
                resp.status()
            )));
        }

        comm.upload(proto::uploader::ResponseUpload {})?;
        Ok(())
    }
}

impl WaitEndState {
    fn run(self, comm: &mut Comm<proto::uploader::Request>) -> Result<State> {
        trace!("wait end state");
        loop {
            let req: proto::uploader::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::End(_) => {
                    comm.end(proto::uploader::ResponseEnd {})?;
                    break;
                }
                _ => {
                    error!("bad request");
                    comm.error(proto::uploader::ResponseError {
                        err: "bad req, waiting end".into(),
                    })?;
                }
            }
        }
        Ok(State::End)
    }
}

pub struct Uploader {
    comm: Comm<proto::uploader::Request>,
    state: State,
}

impl Uploader {
    fn new(comm: Comm<proto::uploader::Request>, tarpath: String) -> Result<Self> {
        log::info!("uploader: {}", tarpath);
        let state = State::Init(InitState { tarpath });
        Ok(Uploader { comm, state })
    }

    fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}, waiting end", err);
                    comm.error(proto::uploader::ResponseError {
                        err: format!("run error: {}", err),
                    })?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}

impl UsbsasProcess for Uploader {
    fn spawn(
        read_fd: RawFd,
        write_fd: RawFd,
        args: Option<Vec<String>>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        if let Some(args) = args {
            if args.len() == 1 {
                Uploader::new(Comm::from_raw_fd(read_fd, write_fd), args[0].to_owned())?
                    .main_loop()
                    .map(|_| log::debug!("uploader exit"))?;
                return Ok(());
            }
        }
        Err(Box::new(Error::Error(
            "uploader need a fname as arg".to_string(),
        )))
    }
}
