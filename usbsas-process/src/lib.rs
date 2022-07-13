//! Helper functions for usbsas processes spawning

use nix::{
    self,
    fcntl::{FcntlArg, FdFlag},
    unistd,
};
use std::{io, os::unix::io::RawFd};
use thiserror::Error;
use usbsas_comm::Comm;

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

pub trait UsbsasProcess {
    fn spawn(
        read_fd: RawFd,
        write_fd: RawFd,
        args: Option<Vec<String>>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>>;
}

pub struct UsbsasChildSpawner {
    args: Option<Vec<String>>,
    wait_on_startup: bool,
}

impl UsbsasChildSpawner {
    pub fn new() -> Self {
        Self {
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

    pub fn wait_on_startup(mut self) -> Self {
        self.wait_on_startup = true;
        self
    }

    pub fn spawn<T: UsbsasProcess, R>(self) -> Result<UsbsasChild<R>> {
        let (child_to_parent_rd, child_to_parent_wr) = unistd::pipe()?;
        let (parent_to_child_rd, parent_to_child_wr) = unistd::pipe()?;

        let child = match unsafe { unistd::fork() } {
            Ok(unistd::ForkResult::Parent { child }) => {
                log::info!(
                    "Spawned child {} with pid {}",
                    std::any::type_name::<T>(),
                    child
                );
                child
            }
            Ok(unistd::ForkResult::Child) => {
                unistd::close(child_to_parent_rd)?;
                unistd::close(parent_to_child_wr)?;
                T::spawn(parent_to_child_rd, child_to_parent_wr, self.args)
                    .map_err(|err| Error::Error(format!("{}", err)))?;
                log::debug!("Child {} exiting", std::any::type_name::<T>());
                std::process::exit(0);
            }
            Err(err) => {
                log::error!(
                    "Failed to fork children {}: {}",
                    std::any::type_name::<T>(),
                    err
                );
                return Err(Error::Error("fork() error".to_string()));
            }
        };

        unistd::close(parent_to_child_rd)?;
        unistd::close(child_to_parent_wr)?;
        Ok(UsbsasChild {
            child,
            comm: Comm::from_raw_fd(child_to_parent_rd, parent_to_child_wr),
            locked: self.wait_on_startup,
        })
    }
}

impl Default for UsbsasChildSpawner {
    fn default() -> Self {
        Self::new()
    }
}

pub struct UsbsasChild<R> {
    pub child: unistd::Pid,
    pub comm: Comm<R>,
    pub locked: bool,
}

impl<R> UsbsasChild<R> {
    pub fn wait(&self) -> Result<()> {
        if let Err(err) = nix::sys::wait::waitpid(self.child, None) {
            log::error!("Couldn't wait child {}: {}", self.child, err);
            return Err(Error::Error("waitpid() error".to_string()));
        }
        Ok(())
    }
}

pub fn pipe() -> io::Result<(RawFd, RawFd)> {
    Ok(nix::unistd::pipe()?)
}

fn fcntl(fd: RawFd, arg: FcntlArg) -> io::Result<libc::c_int> {
    Ok(nix::fcntl::fcntl(fd, arg)?)
}

pub fn close(fd: RawFd) -> io::Result<()> {
    Ok(nix::unistd::close(fd)?)
}

pub fn set_cloexec(fd: RawFd) -> io::Result<libc::c_int> {
    let mut flags = FdFlag::from_bits(fcntl(fd, FcntlArg::F_GETFD)?).unwrap();
    flags.insert(FdFlag::FD_CLOEXEC);
    fcntl(fd, FcntlArg::F_SETFD(flags))
}
