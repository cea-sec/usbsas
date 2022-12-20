use crate::{Error, HttpClient, Result};
use flate2::read::GzDecoder;
use log::{error, trace};
use std::{
    fs::File,
    io::{self, Write},
    os::unix::io::RawFd,
};
use usbsas_comm::{protoresponse, Comm};
use usbsas_config::{conf_parse, conf_read};
use usbsas_process::UsbsasProcess;
use usbsas_proto as proto;
use usbsas_proto::downloader::request::Msg;

protoresponse!(
    CommDownloader,
    downloader,
    download = Download[ResponseDownload],
    downloadstatus = DownloadStatus[ResponseDownloadStatus],
    end = End[ResponseEnd],
    error = Error[ResponseError]
);

struct FileWriterProgress {
    comm: Comm<proto::downloader::Request>,
    file: File,
    filesize: u64,
    offset: u64,
}

impl Write for FileWriterProgress {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let size_written = self.file.write(buf)?;
        self.offset += size_written as u64;
        // if we report progression with each read (of 8kb), the json status of
        // the server polled by the client will quickly become very large and
        // will cause errors. 1 in 10 is enough.
        if (self.offset / size_written as u64) % 10 == 0 || self.offset == self.filesize {
            self.comm
                .downloadstatus(proto::downloader::ResponseDownloadStatus {
                    current_size: self.offset,
                    total_size: self.filesize,
                })?;
        }
        Ok(size_written)
    }
    fn flush(&mut self) -> std::result::Result<(), std::io::Error> {
        self.file.flush()
    }
}

enum State {
    Init(InitState),
    Running(RunningState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut Comm<proto::downloader::Request>) -> Result<Self> {
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
    config_path: String,
}

struct RunningState {
    file: File,
    url: String,
    http_client: HttpClient,
}

struct WaitEndState {}

impl InitState {
    fn run(self, _comm: &mut Comm<proto::downloader::Request>) -> Result<State> {
        let file = File::create(&self.tarpath)?;
        let config_str = conf_read(&self.config_path)?;
        let config = conf_parse(&config_str)?;
        let net_conf = config
            .source_network
            .ok_or_else(|| Error::Error("Missing source network in conf".to_owned()))?;

        Ok(State::Running(RunningState {
            file,
            url: net_conf.url,
            http_client: HttpClient::new(
                #[cfg(feature = "authkrb")]
                net_conf.krb_service_name,
            )?,
        }))
    }
}

impl RunningState {
    fn run(self, comm: &mut Comm<proto::downloader::Request>) -> Result<State> {
        let req: proto::downloader::Request = comm.recv()?;
        match req.msg.ok_or(Error::BadRequest)? {
            Msg::Download(req) => {
                if let Err(err) = self.download(comm, &req.id, req.decompress) {
                    error!("download error: {}", err);
                    comm.error(proto::downloader::ResponseError {
                        err: format!("{}", err),
                    })?;
                };
                Ok(State::WaitEnd(WaitEndState {}))
            }
            Msg::End(_) => {
                comm.end(proto::downloader::ResponseEnd {})?;
                Ok(State::End)
            }
        }
    }

    fn download(
        mut self,
        comm: &mut Comm<proto::downloader::Request>,
        id: &str,
        decompress: bool,
    ) -> Result<()> {
        trace!("download");
        self.url = format!("{}/{}", self.url.trim_end_matches('/'), id);

        let comm_progress = comm.try_clone()?;

        let mut resp = self.http_client.get(&self.url)?;
        if !resp.status().is_success() {
            return Err(Error::Upload(format!(
                "Unknown status code {:?}",
                resp.status()
            )));
        }

        let filesize = resp
            .headers()
            .get("Content-Length")
            .ok_or(Error::BadResponse)?
            .to_str()
            .map_err(|_| Error::BadResponse)?
            .parse::<u64>()
            .map_err(|_| Error::BadResponse)?;
        trace!("file size : {}", filesize);

        let mut filewriterprogress = FileWriterProgress {
            comm: comm_progress,
            file: self.file,
            filesize,
            offset: 0,
        };

        let actual_filesize = match decompress {
            true => {
                let mut gz_decoder = GzDecoder::new(resp);
                std::io::copy(&mut gz_decoder, &mut filewriterprogress)?
            }
            false => resp.copy_to(&mut filewriterprogress)?,
        };

        comm.download(proto::downloader::ResponseDownload {
            filesize: actual_filesize,
        })?;

        Ok(())
    }
}

impl WaitEndState {
    fn run(self, comm: &mut Comm<proto::downloader::Request>) -> Result<State> {
        trace!("wait end state");
        loop {
            let req: proto::downloader::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::End(_) => {
                    comm.end(proto::downloader::ResponseEnd {})?;
                    break;
                }
                _ => {
                    error!("bad request");
                    comm.error(proto::downloader::ResponseError {
                        err: "bad req, waiting end".into(),
                    })?;
                }
            }
        }
        Ok(State::End)
    }
}

pub struct Downloader {
    comm: Comm<proto::downloader::Request>,
    state: State,
}

impl Downloader {
    fn new(
        comm: Comm<proto::downloader::Request>,
        tarpath: String,
        config_path: String,
    ) -> Result<Self> {
        log::info!("downloader: {}", tarpath);
        let state = State::Init(InitState {
            tarpath,
            config_path,
        });
        Ok(Downloader { comm, state })
    }

    fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}, waiting end", err);
                    comm.error(proto::downloader::ResponseError {
                        err: format!("run error: {}", err),
                    })?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}

impl UsbsasProcess for Downloader {
    fn spawn(
        read_fd: RawFd,
        write_fd: RawFd,
        args: Option<Vec<String>>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        if let Some(args) = args {
            if args.len() == 2 {
                Downloader::new(
                    Comm::from_raw_fd(read_fd, write_fd),
                    args[0].to_owned(),
                    args[1].to_owned(),
                )?
                .main_loop()
                .map(|_| log::debug!("downloader exit"))?;
                return Ok(());
            }
        }
        Err(Box::new(Error::Error(
            "downloader need a fname and a config_path arg".to_string(),
        )))
    }
}
