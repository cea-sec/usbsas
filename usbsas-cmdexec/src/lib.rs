//! usbsas cmdexec process.
//!
//! This process will execute the target command specified in the configuration
//! file with the output of the transfer as argument.

use byteorder::ReadBytesExt;
use log::{error, info, trace};
use std::process::{Command, Stdio};
use thiserror::Error;
use usbsas_comm::{ComRpCmdExec, ProtoRespCmdExec, ProtoRespCommon, SendRecv};
use usbsas_config::{conf_parse, conf_read, Command as CmdConf, PostCopy};
use usbsas_proto as proto;
use usbsas_proto::{cmdexec::request::Msg, common::OutFileType};

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("execution error: {0}")]
    Exec(String),
    #[error("Bad Request")]
    BadRequest,
    #[error("No command in configuration file")]
    NoCmdConf,
    #[error("State error")]
    State,
}
pub type Result<T> = std::result::Result<T, Error>;

fn replace_arg_source(args: &[String], out_fname: &str) -> Vec<String> {
    args.iter()
        .map(|arg| match arg.as_ref() {
            "%SOURCE_FILE%" => out_fname.to_owned(),
            _ => arg.to_owned(),
        })
        .collect()
}

enum State {
    Init(InitState),
    Running(RunningState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpCmdExec) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::Running(s) => s.run(comm),
            State::WaitEnd(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {
    out_tar: String,
    out_fs: String,
    config_path: String,
}

struct RunningState {
    out_tar: String,
    out_fs: String,
    cmd: Option<CmdConf>,
    post_copy_cmd: Option<PostCopy>,
}

struct WaitEndState {}

impl InitState {
    fn run(mut self, comm: &mut ComRpCmdExec) -> Result<State> {
        let config = conf_parse(&conf_read(&self.config_path)?)?;

        match comm.read_u8()? {
            // Nothing to do, exit
            0 => return Ok(State::WaitEnd(WaitEndState {})),
            // Use provided tar path
            1 => (),
            // Files of this transfer were analyzed, use clean tar path
            2 => self.out_tar = format!("{}_clean.tar", self.out_tar.trim_end_matches(".tar")),
            _ => {
                error!("bad unlock value");
                return Ok(State::WaitEnd(WaitEndState {}));
            }
        }

        log::trace!("unlocked, using archive {}", self.out_tar);

        Ok(State::Running(RunningState {
            out_tar: self.out_tar,
            out_fs: self.out_fs,
            cmd: config.command,
            post_copy_cmd: config.post_copy,
        }))
    }
}

impl RunningState {
    fn run(mut self, comm: &mut ComRpCmdExec) -> Result<State> {
        loop {
            let req: proto::cmdexec::Request = comm.recv()?;
            let res = match req.msg.ok_or(Error::BadRequest)? {
                Msg::Exec(_) => self.exec(comm),
                Msg::PostCopyExec(req) => self.post_copy(comm, req.outfiletype),
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

    fn exec(&mut self, comm: &mut ComRpCmdExec) -> Result<()> {
        let cmd = self
            .cmd
            .take()
            .ok_or_else(|| Error::Error("No command in conf".to_string()))?;
        let args = replace_arg_source(&cmd.command_args, &self.out_tar);
        self.exec_cmd(cmd.command_bin, args)?;
        comm.exec(proto::cmdexec::ResponseExec {})?;
        Ok(())
    }

    fn post_copy(&mut self, comm: &mut ComRpCmdExec, outft: i32) -> Result<()> {
        let cmd = self.post_copy_cmd.take().ok_or(Error::NoCmdConf)?;
        let outft = OutFileType::try_from(outft).map_err(|_| Error::BadRequest)?;
        let args = match outft {
            OutFileType::Fs => replace_arg_source(&cmd.command_args, &self.out_fs),
            OutFileType::Tar => replace_arg_source(&cmd.command_args, &self.out_tar),
        };
        self.exec_cmd(cmd.command_bin, args)?;
        comm.postcopyexec(proto::cmdexec::ResponsePostCopyExec {})?;
        Ok(())
    }

    fn exec_cmd(&self, binpath: String, args: Vec<String>) -> Result<()> {
        info!("executing {} {:?}", binpath, args);
        match Command::new(binpath)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(cmd) => match cmd.wait_with_output() {
                Ok(output) => {
                    if !output.status.success() {
                        error!("cmd failed");
                        if let Ok(out_result) = String::from_utf8(output.stdout) {
                            error!("cmd stdout: {}", out_result);
                        }
                        if let Ok(err_result) = String::from_utf8(output.stderr) {
                            error!("cmd stderr: {}", err_result);
                        }
                        return Err(Error::Exec("Cmd failed".into()));
                    }
                    Ok(())
                }
                Err(err) => Err(Error::Exec(format!("Can't get cmd result: {err}"))),
            },
            Err(err) => Err(Error::Exec(format!("Failed to start child cmd: {err}"))),
        }
    }
}

impl WaitEndState {
    fn run(self, comm: &mut ComRpCmdExec) -> Result<State> {
        trace!("wait end state");
        loop {
            let req: proto::cmdexec::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::End(_) => {
                    comm.end()?;
                    break;
                }
                _ => {
                    error!("bad request");
                    comm.error("bad req, waiting end")?;
                }
            }
        }
        Ok(State::End)
    }
}

pub struct CmdExec {
    comm: ComRpCmdExec,
    state: State,
}

impl CmdExec {
    pub fn new(
        comm: ComRpCmdExec,
        out_tar: String,
        out_fs: String,
        config_path: String,
    ) -> Result<Self> {
        Ok(CmdExec {
            comm,
            state: State::Init(InitState {
                out_tar,
                out_fs,
                config_path,
            }),
        })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}", err);
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}
