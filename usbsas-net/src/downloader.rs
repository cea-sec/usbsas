use crate::FileWriterProgress;
use crate::{Error, HttpClient, Result};
use log::{error, trace};
use reqwest::Url;
use std::fs::{File, OpenOptions};
use usbsas_comm::{ComRpDownloader, ProtoRespCommon, ProtoRespDownloader};
use usbsas_config::{conf_parse, conf_read};
use usbsas_proto as proto;
use usbsas_proto::{common::Status, downloader::request::Msg};

enum State {
    Init(InitState),
    Running(RunningState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpDownloader) -> Result<Self> {
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
    fn run(self, _comm: &mut ComRpDownloader) -> Result<State> {
        let config = conf_parse(&conf_read(&self.config_path)?)?;
        let net_conf = config.source_network.ok_or(Error::NoConf)?;
        let port = Url::parse(&net_conf.url)?
            .port_or_known_default()
            .ok_or(url::ParseError::InvalidPort)?;

        usbsas_sandbox::net::sandbox(
            Some(
                &[
                    crate::NET_PATHS_RO,
                    #[cfg(feature = "authkrb")]
                    crate::KRB5_PATHS_RO,
                    &[&self.config_path],
                ]
                .concat(),
            ),
            Some(&[&self.tarpath]),
            None,
            Some(&[
                port,
                #[cfg(feature = "authkrb")]
                crate::KRB_AS_PORT,
            ]),
        )?;

        let file = OpenOptions::new()
            .write(true)
            .read(false)
            .open(&self.tarpath)?;

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
    fn run(mut self, comm: &mut ComRpDownloader) -> Result<State> {
        let mut filesize = None;
        loop {
            match comm.recv_req()? {
                Msg::ArchiveInfos(req) => {
                    match self.archive_infos(comm, &req.path) {
                        Ok(size) => filesize = Some(size),
                        Err(err) => {
                            error!("download error: {err}");
                            comm.error(err)?;
                        }
                    };
                }
                Msg::Download(_) => {
                    if let Some(size) = filesize {
                        if let Err(err) = self.download(comm, size) {
                            error!("download error: {err}");
                            comm.error(err)?;
                        };
                        return Ok(State::WaitEnd(WaitEndState {}));
                    } else {
                        comm.error("can't download before knowing the size")?;
                    }
                }
                Msg::End(_) => {
                    comm.end()?;
                    return Ok(State::End);
                }
            }
        }
    }

    fn archive_infos(&mut self, comm: &mut ComRpDownloader, path: &str) -> Result<u64> {
        trace!("req size");
        self.url = format!("{}/{}", self.url.trim_end_matches('/'), path);

        let resp = self.http_client.head(&self.url)?;
        if !resp.status().is_success() {
            return Err(Error::Error(format!(
                "Unknown status code {:?}",
                resp.status()
            )));
        }

        // Even if the archive is gzipped, we're expecting its uncompressed size
        // here
        // XXX TODO check enough space local
        let size = resp
            .headers()
            .get("X-Uncompressed-Content-Length")
            .ok_or(Error::BadResponse)?
            .to_str()
            .map_err(|_| Error::BadResponse)?
            .parse::<u64>()
            .map_err(|_| Error::BadResponse)?;

        trace!("files size: {size}");

        comm.archiveinfos(proto::downloader::ResponseArchiveInfos { size })?;

        Ok(size)
    }

    fn download(mut self, comm: &mut ComRpDownloader, filesize: u64) -> Result<()> {
        trace!("download");
        comm.download(proto::downloader::ResponseDownload {})?;
        let comm_progress =
            ComRpDownloader::new(comm.input().try_clone()?, comm.output().try_clone()?);
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
            status: Status::DlSrc,
        };

        resp.copy_to(&mut filewriterprogress)?;
        comm.done(Status::DlSrc)?;
        Ok(())
    }
}

impl WaitEndState {
    fn run(self, comm: &mut ComRpDownloader) -> Result<State> {
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

pub struct Downloader {
    comm: ComRpDownloader,
    state: State,
}

impl Downloader {
    pub fn new(comm: ComRpDownloader, tarpath: String, config_path: String) -> Result<Self> {
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
                    error!("state run error: {err}, waiting end");
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}
