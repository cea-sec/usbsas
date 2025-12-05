use crate::FileReaderProgress;
use crate::{Error, HttpClient, Result};
use log::{error, trace};
use reqwest::{blocking::Body, Url};
use std::{fs::File, thread::sleep, time::Duration};
use usbsas_comm::{
    ComRpAnalyzer, ComRqJsonParser, ProtoReqCommon, ProtoReqJsonParser, ProtoRespAnalyzer,
    ProtoRespCommon,
};
use usbsas_config::{conf_parse, conf_read};
use usbsas_process::{UsbsasChild, UsbsasChildSpawner};
use usbsas_proto as proto;
use usbsas_proto::{
    analyzer::request::Msg,
    common::{AnalyzeReport, Status},
    jsonparser::SrvResp,
};

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
    json_parser: UsbsasChild<ComRqJsonParser>,
}

struct WaitEndState {}

impl InitState {
    fn run(self, _comm: &mut ComRpAnalyzer) -> Result<State> {
        let conf = if let Some(config) = conf_parse(&conf_read(&self.config_path)?)?.analyzer {
            config
        } else {
            log::warn!("No analyzer conf, parking");
            return Ok(State::WaitEnd(WaitEndState {}));
        };
        let port = Url::parse(&conf.url)?
            .port_or_known_default()
            .ok_or(url::ParseError::InvalidPort)?;
        let json_parser_path = format!("{}/{}", usbsas_utils::USBSAS_BIN_PATH, "usbsas-jsonparser");
        let json_parser =
            UsbsasChildSpawner::new("usbsas-jsonparser").spawn::<ComRqJsonParser>()?;
        usbsas_sandbox::net::sandbox(
            Some(
                &[
                    crate::NET_PATHS_RO,
                    #[cfg(feature = "authkrb")]
                    crate::KRB5_PATHS_RO,
                    &[&self.tarpath, &self.config_path, &json_parser_path],
                ]
                .concat(),
            ),
            None,
            Some(&[&json_parser_path]),
            Some(&[
                port,
                #[cfg(feature = "authkrb")]
                crate::KRB_AS_PORT,
            ]),
        )?;
        let file = File::open(&self.tarpath)?;
        Ok(State::Running(RunningState {
            file: Some(file),
            url: conf.url,
            http_client: HttpClient::new(
                #[cfg(feature = "authkrb")]
                conf.krb_service_name,
            )?,
            json_parser,
        }))
    }
}

impl RunningState {
    fn run(mut self, comm: &mut ComRpAnalyzer) -> Result<State> {
        let mut report = None;
        loop {
            match comm.recv_req()? {
                Msg::Analyze(req) => match self.analyze(comm, &req.id) {
                    Ok(rep) => report = Some(rep),
                    Err(err) => comm.error(err)?,
                },
                Msg::End(_) => {
                    self.json_parser.comm.end()?;
                    comm.end()?;
                    return Ok(State::End);
                }
                Msg::Report(_) => {
                    if let Some(report) = report {
                        comm.report(proto::analyzer::ResponseReport {
                            report: Some(report.clone()),
                        })?;
                        break;
                    } else {
                        comm.error("Files not analyzed yet")?;
                    };
                }
            };
        }
        self.json_parser.comm.end()?;
        Ok(State::WaitEnd(WaitEndState {}))
    }

    fn analyze(&mut self, comm: &mut ComRpAnalyzer, uid: &str) -> Result<AnalyzeReport> {
        trace!("req analyze");
        comm.analyze(proto::analyzer::ResponseAnalyze {})?;

        self.url = format!("{}/{}", self.url.trim_end_matches('/'), uid);

        match self.upload(comm) {
            Ok(res) => {
                trace!("upload for scan result: {:#?}", &res);
                if res.status == "uploaded" {
                    // XXX TODO uploaded but not id returned
                    let id = res.id.ok_or(Error::BadRequest)?;
                    self.url = format!("{}/{}", self.url.trim_end_matches('/'), id);
                }
            }
            Err(err) => {
                error!("upload for scan err: {err}");
                return Err(err);
            }
        }

        let report = self.poll_result()?;

        trace!("analyzer report: {:?}", &report);
        Ok(report)
    }

    fn upload(&mut self, comm: &mut ComRpAnalyzer) -> Result<SrvResp> {
        trace!("upload");
        let file = self.file.take().ok_or(Error::BadRequest)?;
        let filesize = file.metadata()?.len();
        let filereaderprogress = FileReaderProgress {
            comm: ComRpAnalyzer::new(comm.input().try_clone()?, comm.output().try_clone()?),
            file,
            filesize,
            offset: 0,
            status: Status::UploadAv,
        };
        let body = Body::sized(filereaderprogress, filesize);
        trace!("upload to {}", &self.url);
        let resp = self.http_client.post(&self.url, body)?;
        if !resp.status().is_success() {
            return Err(Error::Remote);
        }
        let response = self
            .json_parser
            .comm
            .parseresp(proto::jsonparser::RequestParseResp {
                data: resp.bytes()?.into(),
            })?
            .resp
            .ok_or(Error::Upload("error uploading bundle for analysis".into()))?;
        comm.done(Status::UploadAv)?;
        Ok(response)
    }

    fn poll_result(&mut self) -> Result<AnalyzeReport> {
        trace!("poll result");
        // XXX TODO timeout
        loop {
            trace!("polling {}", &self.url);
            let resp = self.http_client.get(&self.url)?;
            if !resp.status().is_success() {
                return Err(Error::Remote);
            }
            let response_bytes = resp.bytes()?;
            let response = self
                .json_parser
                .comm
                .parseresp(proto::jsonparser::RequestParseResp {
                    data: response_bytes.clone().into(),
                })?
                .resp
                .ok_or(Error::Upload("error getting analysis result".into()))?;

            match response.status.as_str() {
                "scanned" => {
                    let report = self
                        .json_parser
                        .comm
                        .parsereport(proto::jsonparser::RequestParseReport {
                            data: response_bytes.into(),
                        })?
                        .report
                        .ok_or(Error::Upload("error parsing analysis result".into()))?;
                    return Ok(report);
                }
                "uploaded" | "processing" => sleep(Duration::from_secs(1)),
                _ => {
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
                    error!("state run error: {err}, waiting end");
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}
