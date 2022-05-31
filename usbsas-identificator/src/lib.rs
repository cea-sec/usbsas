//! Dummy identificator

use std::os::unix::io::RawFd;
use thiserror::Error;
use usbsas_comm::{protoresponse, Comm};
use usbsas_process::UsbsasProcess;
use usbsas_proto as proto;
use usbsas_proto::identificator::request::Msg;

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("privileges: {0}")]
    Privileges(#[from] usbsas_privileges::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
type Result<T> = std::result::Result<T, Error>;

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
        usbsas_privileges::identificator::drop_priv(comm.input_fd(), comm.output_fd())?;
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
    fn new(comm: Comm<proto::identificator::Request>) -> Result<Self> {
        Ok(Identificator {
            comm,
            state: State::Init(InitState {}),
        })
    }

    fn main_loop(self) -> Result<()> {
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

impl UsbsasProcess for Identificator {
    fn spawn(
        read_fd: RawFd,
        write_fd: RawFd,
        _args: Option<Vec<String>>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        Identificator::new(Comm::from_raw_fd(read_fd, write_fd))?
            .main_loop()
            .map(|_| log::debug!("identificator: exit"))?;
        Ok(())
    }
}
