//! Helper functions for usbsas processes spawning

use nix::{
    self,
    fcntl::{FcntlArg, FdFlag},
    unistd,
};
use std::{
    io,
    os::unix::io::{AsRawFd, RawFd},
    path, process,
};
use thiserror::Error;
use usbsas_utils::{INPUT_PIPE_FD_VAR, OUTPUT_PIPE_FD_VAR, USBSAS_BIN_PATH};

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] io::Error),
    #[error("errno error")]
    Errno(#[from] nix::errno::Errno),
    #[error("{0}")]
    Error(String),
    #[error("Spawn error")]
    Spawn,
}
pub type Result<T> = std::result::Result<T, Error>;

// Default environment variables to keep when spawning processes
const DEFAULT_ENV_VARS: &[&str] = &[
    "TERM",
    "LANG",
    "KRB5CCNAME",
    "PATH",
    "RUST_LOG",
    "RUST_BACKTRACE",
    "USBSAS_SESSION_ID",
    "USBSAS_MOCK_IN_DEV",
    "USBSAS_MOCK_OUT_DEV",
];

pub struct UsbsasChildSpawner<'a> {
    bin_path: &'a str,
    args: Option<Vec<String>>,
    wait_on_startup: bool,
}

impl<'a> UsbsasChildSpawner<'a> {
    pub fn new(bin_path: &'a str) -> Self {
        Self {
            bin_path,
            args: None,
            wait_on_startup: false,
        }
    }

    pub fn arg(mut self, arg: &str) -> Self {
        if let Some(args) = self.args.as_mut() {
            args.push(arg.into())
        } else {
            self.args = Some(vec![arg.into()])
        }
        self
    }

    pub fn args(mut self, args: &[&str]) -> Self {
        if self.args.is_none() {
            self.args = Some(vec![])
        }
        args.iter()
            .for_each(|x| self.args.as_mut().unwrap().push(x.to_string()));
        self
    }

    pub fn wait_on_startup(mut self) -> Self {
        self.wait_on_startup = true;
        self
    }

    pub fn spawn<R: usbsas_comm::ToFromFd + usbsas_comm::SendRecv>(self) -> Result<UsbsasChild<R>> {
        let mut command =
            process::Command::new(path::Path::new(USBSAS_BIN_PATH).join(self.bin_path));

        if let Some(args) = self.args {
            command.args(args);
        }

        let (child_to_parent_rd, child_to_parent_wr) = unistd::pipe()?;
        let (parent_to_child_rd, parent_to_child_wr) = unistd::pipe()?;
        set_cloexec(child_to_parent_rd.as_raw_fd())?;
        set_cloexec(parent_to_child_wr.as_raw_fd())?;

        command.env_clear();
        command.env(
            INPUT_PIPE_FD_VAR,
            parent_to_child_rd.as_raw_fd().to_string(),
        );
        command.env(
            OUTPUT_PIPE_FD_VAR,
            child_to_parent_wr.as_raw_fd().to_string(),
        );
        DEFAULT_ENV_VARS
            .iter()
            .map(|k| std::env::var(k).map(|v| (k, v)))
            .filter_map(std::result::Result::ok)
            .for_each(|(k, v)| {
                command.env(k, v);
            });

        let child = command.spawn()?;

        drop(parent_to_child_rd);
        drop(child_to_parent_wr);

        Ok(UsbsasChild {
            child,
            comm: R::from_fd(child_to_parent_rd, parent_to_child_wr),
            locked: self.wait_on_startup,
        })
    }
}

pub struct UsbsasChild<R: usbsas_comm::SendRecv> {
    pub child: process::Child,
    pub comm: R,
    pub locked: bool,
}

impl<R: usbsas_comm::SendRecv> UsbsasChild<R> {
    pub fn wait(&mut self) -> Result<std::process::ExitStatus> {
        Ok(self.child.wait()?)
    }

    pub fn unlock_with(&mut self, buf: &[u8]) -> Result<()> {
        if !self.locked {
            return Err(Error::Error("not locked".into()));
        }
        self.comm.write_all(buf)?;
        self.locked = false;
        Ok(())
    }
}

fn fcntl(fd: RawFd, arg: FcntlArg) -> io::Result<libc::c_int> {
    Ok(nix::fcntl::fcntl(fd, arg)?)
}

pub fn set_cloexec(fd: RawFd) -> io::Result<libc::c_int> {
    let mut flags = FdFlag::from_bits(fcntl(fd, FcntlArg::F_GETFD)?).unwrap();
    flags.insert(FdFlag::FD_CLOEXEC);
    fcntl(fd, FcntlArg::F_SETFD(flags))
}
