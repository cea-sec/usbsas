//! usbsas communication helper struct and functions.
//!
//! Protobuf messages are encoded / decoded here. Messages are all prefixed with
//! the size of the message (64 bit LE).

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::{
    env,
    fs::File,
    io::{self, Read, Write},
    marker::PhantomData,
    os::unix::io::{AsRawFd, FromRawFd, RawFd},
    str::FromStr,
};
use usbsas_utils::{INPUT_PIPE_FD_VAR, OUTPUT_PIPE_FD_VAR};

pub trait FromEnv: Sized {
    type Err;
    fn from_env(name: &str) -> Result<Self, Self::Err>;
}

impl FromEnv for RawFd {
    type Err = io::Error;
    fn from_env(name: &str) -> Result<Self, Self::Err> {
        env::var(name)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))
            .and_then(|value| {
                RawFd::from_str(&value).map_err(|err| io::Error::new(io::ErrorKind::Other, err))
            })
    }
}

/// Struct containing input (read) and output (write) communication pipes.
/// Comm is marked with `PhantomData` on the type of protobuf messages it will
/// send / recv.
pub struct Comm<R> {
    input: File,
    output: File,
    req: PhantomData<R>,
}

impl<R> Comm<R> {
    pub fn new(input: File, output: File) -> Self {
        Comm {
            input,
            output,
            req: PhantomData,
        }
    }

    pub fn input_fd(&self) -> RawFd {
        self.input.as_raw_fd()
    }

    pub fn output_fd(&self) -> RawFd {
        self.output.as_raw_fd()
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Comm::new(self.input.try_clone()?, self.output.try_clone()?))
    }

    /// Instantiate `Comm` with file descriptors from environment variables
    /// `INPUT_PIPE_FD_VAR` and `OUTPUT_PIPE_FD_VAR`.
    pub fn from_env() -> io::Result<Self> {
        let pipe_in = RawFd::from_env(INPUT_PIPE_FD_VAR)?;
        let pipe_out = RawFd::from_env(OUTPUT_PIPE_FD_VAR)?;
        Ok(Comm {
            input: unsafe { File::from_raw_fd(pipe_in) },
            output: unsafe { File::from_raw_fd(pipe_out) },
            req: PhantomData,
        })
    }

    pub fn from_raw_fd(read: RawFd, write: RawFd) -> Self {
        let output;
        let input;
        unsafe {
            input = File::from_raw_fd(read);
            output = File::from_raw_fd(write);
        }
        Comm {
            input,
            output,
            req: PhantomData,
        }
    }

    pub fn send<T: prost::Message>(&mut self, req: T) -> io::Result<()> {
        // Send length, on 8 bytes
        self.output
            .write_u64::<LittleEndian>(req.encoded_len() as u64)?;

        // Encode request
        let mut buf = Vec::new();
        buf.reserve(req.encoded_len());
        req.encode(&mut buf)?;

        // Send request
        self.output.write_all(&buf)?;
        Ok(())
    }

    pub fn recv<T>(&mut self) -> io::Result<T>
    where
        T: prost::Message,
        T: std::default::Default,
    {
        let len = self.input.read_u64::<LittleEndian>()?;

        let mut buf = vec![0; len as usize];
        self.input.read_exact(&mut buf)?;
        Ok(prost::Message::decode(buf.as_slice())?)
    }
}

impl<R> Write for Comm<R> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

impl<R> Read for Comm<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf)
    }
}

/// This macro defines a trait and implements it on Comm<R> to facilitate
/// sending a request and receiving its corresponding response.
///
/// For example with usbdev:
///
/// usbdev.proto3 message definition:
///
/// ```ignore
///
/// message RequestDevices {};
/// message RequestEnd {};
///
/// message Request {
///   oneof msg {
///     RequestEnd End = 1;
///     RequestDevices Devices = 2;
///   }
/// };
///
/// message ResponseDevices {
///   repeated common.Device devices = 1;
/// };
///
/// message ResponseEnd {};
///
/// message ResponseError {
///   string err = 1;
/// };
///
/// message Response {
///   oneof msg {
///     ResponseEnd End = 1;
///     ResponseError Error = 2;
///     ResponseDevices Devices = 3;
///   }
/// };
///
/// ```
///
/// In usbdev.rs:
/// ```rust,ignore
/// protorequest!(
///    CommUsbdev,
///    usbdev,
///    devices = Devices[RequestDevices, ResponseDevices],
///    end = End[RequestEnd, ResponseEnd]
/// );
///
/// let comm : Comm<proto::usbdev::Request> = Comm::from_env()?;
///
/// // Send RequestDevices and receive a ResponseDevices
/// let devices = comm.devices()?.devices;
///
/// ```
#[macro_export]
macro_rules! protorequest {
    ($trait:ident,
     $mod: ident,
     $($name:ident = $rtype:ident[$req:ident, $resp:ident] ),+) => {
        pub trait $trait {
            $(
                fn $name(&mut self, req: proto::$mod::$req) -> std::io::Result<proto::$mod::$resp>;
            )+
        }
        impl $trait for Comm<proto::$mod::Request> {
            $(
                fn $name(&mut self, req: proto::$mod::$req) -> std::io::Result<proto::$mod::$resp> {
                    let req_parts = proto::$mod::Request {
                        msg: Some(proto::$mod::request::Msg::$rtype(req)),
                    };
                    self.send(req_parts)?;
                    let resp: proto::$mod::Response = self.recv()?;
                    match resp.msg {
                        Some(proto::$mod::response::Msg::$rtype(info)) => {
                            Ok(info)
                        }
                        Some(proto::$mod::response::Msg::Error(e)) => {
                            Err(std::io::Error::new(std::io::ErrorKind::Other, e.err))
                        }
                        _ => Err(std::io::Error::new(std::io::ErrorKind::Other, "Bad response type"))
                    }
                }
            )+
        }
    };
}

/// Same as `protorequest` but for answering Responses.
#[macro_export]
macro_rules! protoresponse {
    ($trait:ident,
     $mod: ident,
     $($name:ident = $rtype:ident[$resp:ident] ),+) => {
        pub trait $trait {
            $(
                fn $name(&mut self, resp: proto::$mod::$resp) -> std::io::Result<()>;
            )+
        }
        impl $trait for Comm<proto::$mod::Request> {
            $(
                fn $name(&mut self, resp: proto::$mod::$resp) -> std::io::Result<()> {
                    self.send(proto::$mod::Response {
                        msg: Some(proto::$mod::response::Msg::$rtype(resp)),
                    })
                }
            )+
        }
    };
}
