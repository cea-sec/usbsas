use crate::{Error, Result};
use log::{error, trace};
use usbsas_comm::{ComRpJsonParser, ProtoRespCommon, ProtoRespJsonParser, ToFd};
use usbsas_proto::{
    self as proto,
    common::AnalyzeReport,
    jsonparser::{request::Msg, SrvResp},
};

enum State {
    Init(InitState),
    Running(RunningState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpJsonParser) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::Running(s) => s.run(comm),
            State::WaitEnd(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {}

struct RunningState {}

struct WaitEndState {}

impl InitState {
    fn run(self, comm: &mut ComRpJsonParser) -> Result<State> {
        usbsas_sandbox::jsonparser::seccomp(comm.input_fd(), comm.output_fd())?;
        Ok(State::Running(RunningState {}))
    }
}

impl RunningState {
    fn run(self, comm: &mut ComRpJsonParser) -> Result<State> {
        trace!("wait end state");
        loop {
            match comm.recv_req()? {
                Msg::ParseResp(req) => {
                    match serde_json::from_slice::<SrvResp>(&req.data) {
                        Ok(resp) => {
                            comm.parseresp(proto::jsonparser::ResponseParseResp {
                                resp: Some(resp.clone()),
                            })?;
                        }
                        Err(err) => {
                            error!("couldn't parse resp: {}", err);
                            comm.error("couldn't parse response from server")?;
                        }
                    };
                }
                Msg::ParseReport(req) => {
                    match serde_json::from_slice::<AnalyzeReport>(&req.data) {
                        Ok(report) => {
                            comm.parsereport(proto::jsonparser::ResponseParseReport {
                                report: Some(report.clone()),
                            })?;
                        }
                        Err(err) => {
                            error!("couldn't parse resp: {}", err);
                            comm.error("couldn't parse analyze report from server")?;
                        }
                    }
                    break;
                }
                Msg::End(_) => {
                    comm.end()?;
                    return Ok(State::End);
                }
            }
        }
        Ok(State::WaitEnd(WaitEndState {}))
    }
}

impl WaitEndState {
    fn run(self, comm: &mut ComRpJsonParser) -> Result<State> {
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

pub struct JsonParser {
    comm: ComRpJsonParser,
    state: State,
}

impl JsonParser {
    pub fn new(comm: ComRpJsonParser) -> Result<Self> {
        let state = State::Init(InitState {});
        Ok(JsonParser { comm, state })
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
