use crate::{Error, HttpClient, Result};
use log::{error, trace};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
};
use usbsas_comm::{protoresponse, Comm};
use usbsas_config::{conf_parse, conf_read};
use usbsas_proto as proto;
use usbsas_proto::downloader::request::Msg;

protoresponse!(
    CommDownloader,
    downloader,
    download = Download[ResponseDownload],
    downloadstatus = DownloadStatus[ResponseDownloadStatus],
    archiveinfos = ArchiveInfos[ResponseArchiveInfos],
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
        usbsas_sandbox::landlock(
            Some(&[
                &self.config_path,
                "/etc",
                "/lib",
                "/usr/lib/",
                "/var/lib/usbsas",
            ]),
            Some(&[&self.tarpath]),
        )?;

        let file = OpenOptions::new()
            .write(true)
            .read(false)
            .open(&self.tarpath)?;
        let config_str = conf_read(&self.config_path)?;
        let config = conf_parse(&config_str)?;
        let net_conf = config.source_network.ok_or(Error::NoConf)?;

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
    fn run(mut self, comm: &mut Comm<proto::downloader::Request>) -> Result<State> {
        let mut filesize = None;
        loop {
            let req: proto::downloader::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::ArchiveInfos(req) => {
                    match self.archive_infos(comm, &req.id) {
                        Ok(size) => filesize = Some(size),
                        Err(err) => {
                            error!("download error: {}", err);
                            comm.error(proto::downloader::ResponseError {
                                err: format!("{err}"),
                            })?;
                        }
                    };
                }
                Msg::Download(_) => {
                    if let Some(size) = filesize {
                        if let Err(err) = self.download(comm, size) {
                            error!("download error: {}", err);
                            comm.error(proto::downloader::ResponseError {
                                err: format!("{err}"),
                            })?;
                        };
                        return Ok(State::WaitEnd(WaitEndState {}));
                    } else {
                        comm.error(proto::downloader::ResponseError {
                            err: "can't download before knowing the size".to_owned(),
                        })?;
                    }
                }
                Msg::End(_) => {
                    comm.end(proto::downloader::ResponseEnd {})?;
                    return Ok(State::End);
                }
            }
        }
    }

    fn archive_infos(
        &mut self,
        comm: &mut Comm<proto::downloader::Request>,
        id: &str,
    ) -> Result<u64> {
        trace!("req size");
        self.url = format!("{}/{}", self.url.trim_end_matches('/'), id);

        let resp = self.http_client.head(&self.url)?;
        if !resp.status().is_success() {
            return Err(Error::Error(format!(
                "Unknown status code {:?}",
                resp.status()
            )));
        }

        // Even if the archive is gzipped, we're expecting its uncompressed size
        // here
        let size = resp
            .headers()
            .get("X-Uncompressed-Content-Length")
            .ok_or(Error::BadResponse)?
            .to_str()
            .map_err(|_| Error::BadResponse)?
            .parse::<u64>()
            .map_err(|_| Error::BadResponse)?;

        trace!("files size: {}", size);

        comm.archiveinfos(proto::downloader::ResponseArchiveInfos { size })?;

        Ok(size)
    }

    fn download(
        mut self,
        comm: &mut Comm<proto::downloader::Request>,
        filesize: u64,
    ) -> Result<()> {
        trace!("download");
        let comm_progress = comm.try_clone()?;
        let mut resp = self.http_client.get(&self.url)?;
        if !resp.status().is_success() {
            return Err(Error::Error(format!(
                "Unknown status code {:?}",
                resp.status()
            )));
        }

        let mut filewriterprogress = FileWriterProgress {
            comm: comm_progress,
            file: self.file,
            filesize,
            offset: 0,
        };

        resp.copy_to(&mut filewriterprogress)?;
        comm.download(proto::downloader::ResponseDownload {})?;
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
    pub fn new(
        comm: Comm<proto::downloader::Request>,
        tarpath: String,
        config_path: String,
    ) -> Result<Self> {
        let state = State::Init(InitState {
            tarpath,
            config_path,
        });
        Ok(Downloader { comm, state })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(Error::NoConf) => {
                    log::warn!("No configuration for downloader, parking");
                    State::WaitEnd(WaitEndState {})
                }
                Err(err) => {
                    error!("state run error: {}, waiting end", err);
                    comm.error(proto::downloader::ResponseError {
                        err: format!("run error: {err}"),
                    })?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}
