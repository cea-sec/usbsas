use crate::{Error, HttpClient, Result};
use log::{error, trace};
use reqwest::blocking::Body;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read},
    thread::sleep,
    time::Duration,
};
use usbsas_comm::{ComRpAnalyzer, ProtoRespAnalyzer, ProtoRespCommon, SendRecv};
use usbsas_config::{conf_parse, conf_read};
use usbsas_proto as proto;
use usbsas_proto::analyzer::request::Msg;

#[derive(Debug, Serialize, Deserialize)]
struct JsonRes {
    status: String,
    id: String,
    files: Option<HashMap<String, serde_json::Value>>,
}

struct FileReaderProgress {
    comm: ComRpAnalyzer,
    file: File,
    pub filesize: u64,
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
            self.comm.status(self.offset, self.filesize, false)?;
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
    fn run(self, comm: &mut ComRpAnalyzer) -> Result<Self> {
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
    file: Option<File>,
    url: String,
    http_client: HttpClient,
}

struct WaitEndState {}

impl InitState {
    fn run(self, _comm: &mut ComRpAnalyzer) -> Result<State> {
        usbsas_sandbox::landlock(
            Some(&[
                &self.tarpath,
                &self.config_path,
                "/etc",
                "/lib",
                "/usr/lib/",
                "/var/lib/usbsas",
            ]),
            None,
        )?;

        let file = File::open(&self.tarpath)?;
        let config = conf_parse(&conf_read(&self.config_path)?)?;

        // XXX seccomp

        if let Some(conf) = config.analyzer {
            Ok(State::Running(RunningState {
                file: Some(file),
                url: conf.url,
                http_client: HttpClient::new(
                    #[cfg(feature = "authkrb")]
                    conf.krb_service_name,
                )?,
            }))
        } else {
            log::warn!("No analyzer conf, parking");
            Ok(State::WaitEnd(WaitEndState {}))
        }
    }
}

impl RunningState {
    fn run(mut self, comm: &mut ComRpAnalyzer) -> Result<State> {
        loop {
            let req: proto::analyzer::Request = comm.recv()?;
            let res = match req.msg.ok_or(Error::BadRequest)? {
                Msg::Analyze(req) => self.analyze(comm, &req.id),
                Msg::End(_) => {
                    comm.end()?;
                    break;
                }
            };
            match res {
                Ok(_) => continue,
                Err(err) => {
                    error!("{}", err);
                    comm.error(err)?;
                }
            }
        }
        Ok(State::End)
    }

    fn analyze(&mut self, comm: &mut ComRpAnalyzer, uid: &str) -> Result<()> {
        trace!("req analyze");

        self.url = format!("{}/{}", self.url.trim_end_matches('/'), uid);

        match self.upload(comm) {
            Ok(res) => {
                trace!("upload for scan result: {:#?}", &res);
                if res.status == "uploaded" {
                    self.url = format!("{}/{}", self.url.trim_end_matches('/'), res.id);
                }
            }
            Err(err) => {
                error!("upload for scan err: {}", err);
                return Err(err);
            }
        }

        let report = self.poll_result()?;

        trace!("analyzer report: {}", &report);
        comm.analyze(proto::analyzer::ResponseAnalyze { report })?;
        Ok(())
    }

    fn upload(&mut self, comm: &mut ComRpAnalyzer) -> Result<JsonRes> {
        trace!("upload");
        let file = self.file.take().ok_or(Error::BadRequest)?;
        let filesize = file.metadata()?.len();
        let filereaderprogress = FileReaderProgress {
            comm: comm.try_clone()?,
            file,
            filesize,
            offset: 0,
        };
        let body = Body::sized(filereaderprogress, filesize);
        trace!("upload to {}", &self.url);
        let resp = self.http_client.post(&self.url, body)?;
        if !resp.status().is_success() {
            return Err(Error::Remote);
        }
        Ok(resp.json()?)
    }

    fn poll_result(&mut self) -> Result<String> {
        trace!("poll result");
        // XXX TODO timeout
        loop {
            trace!("polling {}", &self.url);
            let resp = self.http_client.get(&self.url)?;
            if !resp.status().is_success() {
                return Err(Error::Remote);
            }
            let raw_report = resp.text()?;
            let report: JsonRes = serde_json::from_str(&raw_report)?;
            trace!("res: {:#?}", &report);
            match report.status.as_str() {
                "scanned" => return Ok(raw_report),
                "uploaded" | "processing" => sleep(Duration::from_secs(1)),
                _ => {
                    log::error!("{report:?}");
                    return Err(Error::Remote);
                }
            }
        }
    }
}

impl WaitEndState {
    fn run(self, comm: &mut ComRpAnalyzer) -> Result<State> {
        trace!("wait end state");
        loop {
            let req: proto::analyzer::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
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

pub struct Analyzer {
    comm: ComRpAnalyzer,
    state: State,
}

impl Analyzer {
    pub fn new(comm: ComRpAnalyzer, tarpath: String, config_path: String) -> Result<Self> {
        let state = State::Init(InitState {
            tarpath,
            config_path,
        });
        Ok(Analyzer { comm, state })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}, waiting end", err);
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}
