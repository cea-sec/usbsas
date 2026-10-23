//! Helper functions for usbsas processes spawning
//!
//! [`UsbsasChildSpawner`] starts a usbsas binary connected to its parent with
//! two pipes and returns a [`UsbsasChild`], whose `comm` is used to send it
//! requests. [`ChildMngt`] handles the lifecycle of the child (unlock, end,
//! wait).

use nix::{
    self,
    fcntl::{FcntlArg, FdFlag},
    unistd,
};
use std::{
    io,
    os::unix::io::{AsFd, AsRawFd},
    path, process,
};
use thiserror::Error;
use usbsas_utils::{INPUT_PIPE_FD_VAR, OUTPUT_PIPE_FD_VAR, USBSAS_BIN_PATH};

/// Errors of this crate.
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
/// Result type of this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Default environment variables to keep when spawning processes, the rest of
/// the environment is cleared.
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

/// Builder used to spawn a usbsas child process.
pub struct UsbsasChildSpawner<'a> {
    /// Binary to execute, relative to `USBSAS_BIN_PATH`
    bin_path: &'a str,
    args: Option<Vec<String>>,
    /// Whether the child waits to be unlocked after startup
    wait_on_startup: bool,
}

impl<'a> UsbsasChildSpawner<'a> {
    /// New spawner for the binary `bin_path`, relative to `USBSAS_BIN_PATH`
    /// (e.g. `"usbsas-tar2files"`).
    pub fn new(bin_path: &'a str) -> Self {
        Self {
            bin_path,
            args: None,
            wait_on_startup: false,
        }
    }

    /// Add a command line argument.
    pub fn arg(mut self, arg: &str) -> Self {
        if let Some(args) = self.args.as_mut() {
            args.push(arg.into())
        } else {
            self.args = Some(vec![arg.into()])
        }
        self
    }

    /// Add command line arguments.
    pub fn args(mut self, args: &[&str]) -> Self {
        if self.args.is_none() {
            self.args = Some(vec![])
        }
        args.iter()
            .for_each(|x| self.args.as_mut().unwrap().push(x.to_string()));
        self
    }

    /// Spawn the child locked: it waits for a value from the parent before
    /// starting (see [`ChildMngt::unlock_with`]). The child must implement
    /// this wait itself.
    pub fn wait_on_startup(mut self) -> Self {
        self.wait_on_startup = true;
        self
    }

    /// Spawn the child.
    ///
    /// Two pipes are created, one per direction. The child gets its ends via
    /// the `INPUT_PIPE_FD` and `OUTPUT_PIPE_FD` environment variables, the
    /// parent's ends are close-on-exec and wrapped into a communication channel
    /// of type `R` (e.g. `ComRqFiles`). The child's environment only keeps
    /// a few allowed variables (`PATH`, `RUST_LOG`, `USBSAS_SESSION_ID`...).
    pub fn spawn<R: usbsas_comm::FromFd + usbsas_comm::ProtoReqCommon>(
        self,
    ) -> Result<UsbsasChild<R>> {
        let mut command =
            process::Command::new(path::Path::new(USBSAS_BIN_PATH).join(self.bin_path));

        if let Some(args) = self.args {
            command.args(args);
        }

        let (child_to_parent_rd, child_to_parent_wr) = unistd::pipe()?;
        let (parent_to_child_rd, parent_to_child_wr) = unistd::pipe()?;
        set_cloexec(&child_to_parent_rd)?;
        set_cloexec(&parent_to_child_wr)?;

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

/// A spawned usbsas child process.
///
/// The process isn't killed when this is dropped, it should be ended with
/// [`ChildMngt::end`] and waited with [`ChildMngt::wait`].
pub struct UsbsasChild<R: usbsas_comm::ProtoReqCommon> {
    pub child: process::Child,
    /// Channel to send requests to the child
    pub comm: R,
    /// The child is waiting to be unlocked
    pub locked: bool,
}

/// Lifecycle management of a child, object safe to handle children of different
/// types together.
pub trait ChildMngt {
    /// Wait for the child to exit.
    fn wait(&mut self) -> Result<std::process::ExitStatus>;
    /// Unlock a child spawned with [`UsbsasChildSpawner::wait_on_startup`] by
    /// writing `value` (as a little endian `u64`) on its pipe. The meaning of
    /// `value` depends on the child, `0` means it won't be used.
    ///
    /// Returns an error if the child isn't locked.
    fn unlock_with(&mut self, value: u64) -> Result<()>;
    /// Unlock the child with `0` if needed, then send it an `End` request and
    /// wait for the response.
    fn end(&mut self) -> Result<()>;
}

impl<R: usbsas_comm::ProtoReqCommon> ChildMngt for UsbsasChild<R> {
    fn wait(&mut self) -> Result<std::process::ExitStatus> {
        Ok(self.child.wait()?)
    }
    fn unlock_with(&mut self, value: u64) -> Result<()> {
        if !self.locked {
            return Err(Error::Error("not locked".into()));
        }
        self.comm.write_all(&value.to_le_bytes())?;
        self.locked = false;
        Ok(())
    }
    fn end(&mut self) -> Result<()> {
        if self.locked {
            self.unlock_with(0)?;
            self.locked = false;
        }
        Ok(self.comm.end()?)
    }
}

/// Set the `FD_CLOEXEC` flag on a file descriptor.
pub fn set_cloexec<Fd: AsFd>(fd: Fd) -> io::Result<libc::c_int> {
    let mut flags = FdFlag::from_bits(nix::fcntl::fcntl(&fd, FcntlArg::F_GETFD)?).unwrap();
    flags.insert(FdFlag::FD_CLOEXEC);
    Ok(nix::fcntl::fcntl(fd, FcntlArg::F_SETFD(flags))?)
}
