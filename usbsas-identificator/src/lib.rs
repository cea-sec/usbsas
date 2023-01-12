//! Dummy identificator

use thiserror::Error;
use usbsas_comm::{protoresponse, Comm};
use usbsas_proto as proto;
use usbsas_proto::identificator::request::Msg;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
pub type Result<T> = std::result::Result<T, Error>;

protoresponse!(
    CommIdentificator,
    identificator,
    id = Id[ResponseId],
    error = Error[ResponseError],
    end = End[ResponseEnd]
);

enum State {
    Init(InitState),
    Running(RunningState),
    End,
}

impl State {
    fn run(self, comm: &mut Comm<proto::identificator::Request>) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::Running(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {}

struct RunningState {
    current_id: Option<String>,
}

impl InitState {
    fn run(self, comm: &mut Comm<proto::identificator::Request>) -> Result<State> {
        usbsas_sandbox::identificator::seccomp(comm.input_fd(), comm.output_fd())?;
        Ok(State::Running(RunningState { current_id: None }))
    }
}

impl RunningState {
    fn run(mut self, comm: &mut Comm<proto::identificator::Request>) -> Result<State> {
        loop {
            let req: proto::identificator::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::Id(_) => {
                    let id = self.get_id()?;
                    comm.id(proto::identificator::ResponseId { id })?;
                }
                Msg::End(_) => {
                    comm.end(proto::identificator::ResponseEnd {})?;
                    break;
                }
            }
        }
        Ok(State::End)
    }

    fn get_id(&mut self) -> Result<String> {
        if let Some(id) = &self.current_id {
            Ok(id.to_string())
        } else {
            let new_id = String::from("Tartempion");
            self.current_id = Some(new_id.clone());
            Ok(new_id)
        }
    }
}

pub struct Identificator {
    comm: Comm<proto::identificator::Request>,
    state: State,
}

impl Identificator {
    pub fn new(comm: Comm<proto::identificator::Request>) -> Result<Self> {
        Ok(Identificator {
            comm,
            state: State::Init(InitState {}),
        })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm)? {
                State::End => break,
                state => state,
            }
        }
        Ok(())
    }
}
