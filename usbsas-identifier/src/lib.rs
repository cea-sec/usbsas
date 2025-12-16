//! Dummy identifier

use thiserror::Error;
use usbsas_comm::{ComRpIdentifier, ProtoRespCommon, ProtoRespIdentifier, ToFd};
use usbsas_proto::{self as proto, identifier::request::Msg};

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

enum State {
    Init(InitState),
    Running(RunningState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpIdentifier) -> Result<Self> {
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
    fn run(self, comm: &mut ComRpIdentifier) -> Result<State> {
        usbsas_sandbox::identifier::seccomp(comm.input_fd(), comm.output_fd())?;
        Ok(State::Running(RunningState { current_id: None }))
    }
}

impl RunningState {
    fn run(mut self, comm: &mut ComRpIdentifier) -> Result<State> {
        loop {
            match comm.recv_req()? {
                Msg::UserId(_) => {
                    let userid = self.get_id()?;
                    comm.userid(proto::identifier::ResponseUserId { userid })?;
                }
                Msg::End(_) => {
                    comm.end()?;
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

pub struct Identifier {
    comm: ComRpIdentifier,
    state: State,
}

impl Identifier {
    pub fn new(comm: ComRpIdentifier) -> Result<Self> {
        Ok(Identifier {
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
