//! usbsas communication helper struct and functions.
//!
//! Protobuf messages are encoded / decoded here. Messages are all prefixed with
//! the size of the message (64 bit LE).

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use paste::paste;
use std::{
    env,
    fs::File,
    io::{self, Read, Write},
    marker::PhantomData,
    os::{
        fd::OwnedFd,
        unix::io::{AsRawFd, FromRawFd, RawFd},
    },
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

pub trait FromFd {
    fn from_fd(read: OwnedFd, write: OwnedFd) -> Self;
}
impl<U, R: Read + From<OwnedFd>, W: Write + From<OwnedFd>> FromFd for Comm<U, R, W> {
    fn from_fd(inp: OwnedFd, out: OwnedFd) -> Self {
        Comm {
            input: R::from(inp),
            output: W::from(out),
            req: PhantomData,
        }
    }
}

pub trait ToFd {
    fn input_fd(&self) -> RawFd;
    fn output_fd(&self) -> RawFd;
}
impl<U, R: Read + AsRawFd, W: Write + AsRawFd> ToFd for Comm<U, R, W> {
    fn input_fd(&self) -> RawFd {
        self.input.as_raw_fd()
    }
    fn output_fd(&self) -> RawFd {
        self.output.as_raw_fd()
    }
}

/// Typed struct containing input (read) and output (write) communication pipes.
/// Comm is marked with `PhantomData` on the type of protobuf messages it will
/// send / recv.
pub struct Comm<U, R: Read, W: Write> {
    input: R,
    output: W,
    req: PhantomData<U>,
}

impl<U, R: Read + AsRawFd + From<OwnedFd>, W: Write + AsRawFd + From<OwnedFd>> Comm<U, R, W> {
    pub fn new(input: R, output: W) -> Self {
        Comm {
            input,
            output,
            req: PhantomData,
        }
    }

    /// Instantiate `Comm` with file descriptors from environment variables
    /// `INPUT_PIPE_FD_VAR` and `OUTPUT_PIPE_FD_VAR`.
    pub fn from_env() -> io::Result<Comm<U, R, W>> {
        Ok(Self::from_fd(
            unsafe { OwnedFd::from_raw_fd(RawFd::from_env(INPUT_PIPE_FD_VAR)?) },
            unsafe { OwnedFd::from_raw_fd(RawFd::from_env(OUTPUT_PIPE_FD_VAR)?) },
        ))
    }

    pub fn input(&self) -> &R {
        &self.input
    }

    pub fn output(&self) -> &W {
        &self.output
    }
}

impl<U, R: Read, W: Write> Write for Comm<U, R, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

impl<U, R: Read, W: Write> Read for Comm<U, R, W> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf)
    }
}

pub trait SendRecv: Read + Write {
    fn send<T: prost::Message>(&mut self, req: T) -> io::Result<()> {
        // Send length, on 8 bytes
        self.write_u64::<LittleEndian>(req.encoded_len() as u64)?;

        // Encode request
        let mut buf = Vec::with_capacity(req.encoded_len());
        req.encode(&mut buf)?;

        // Send request
        self.write_all(&buf)?;
        Ok(())
    }

    fn recv<T>(&mut self) -> io::Result<T>
    where
        T: prost::Message,
        T: std::default::Default,
    {
        let len = self.read_u64::<LittleEndian>()?;

        let mut buf = vec![0; len as usize];
        self.read_exact(&mut buf)?;
        Ok(prost::Message::decode(buf.as_slice())?)
    }
}

/// Common function that requester processes must implement
pub trait ProtoReqCommon: SendRecv {
    fn end(&mut self) -> std::io::Result<()>;
    fn recv_status(&mut self) -> std::io::Result<usbsas_proto::common::ResponseStatus>;
}

/// Common functions that responder processes must implement
pub trait ProtoRespCommon: SendRecv {
    fn status(
        &mut self,
        current: u64,
        total: u64,
        done: bool,
        status: usbsas_proto::common::Status,
    ) -> std::io::Result<()>;
    fn error(&mut self, err: impl std::string::ToString) -> std::io::Result<()>;
    fn end(&mut self) -> std::io::Result<()>;
    fn done(&mut self, status: usbsas_proto::common::Status) -> std::io::Result<()>;
}

impl<U, R: Read, W: Write> SendRecv for Comm<U, R, W> {}

/// This macro defines a trait and implements it on Comm<U> to facilitate
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
/// protorequest!(Usbdev, Devices, End);
///
/// let comm: CommUsbdev = Comm::from_env()?;
///
/// // Send RequestDevices and receive a ResponseDevices
/// let devices = comm.devices()?.devices;
///
/// ```
#[macro_export]
macro_rules! protorequest {
    ($proto:ident,
     $($req:ident),+) => {
        paste! {
            pub type [<ComRq$proto>] = Comm<usbsas_proto::[<$proto:lower>]::Request, File, File>;
            pub trait [<ProtoReq$proto>]: SendRecv + ProtoReqCommon {
                $(
                    fn [<$req:lower>](&mut self, req: usbsas_proto::[<$proto:lower>]::[<Request$req>]) -> std::io::Result<usbsas_proto::[<$proto:lower>]::[<Response$req>]>;
                )+
            }
            impl<R: Read, W: Write> ProtoReqCommon for Comm<usbsas_proto::[<$proto:lower>]::Request, R, W> {
                fn end(&mut self) -> std::io::Result<()> {
                    let req_parts = usbsas_proto::[<$proto:lower>]::Request {
                        msg: Some(usbsas_proto::[<$proto:lower>]::request::Msg::End(usbsas_proto::[<$proto:lower>]::RequestEnd {})),
                    };
                    self.send(req_parts)?;
                    let resp: usbsas_proto::[<$proto:lower>]::Response = self.recv()?;
                    match resp.msg {
                        Some(usbsas_proto::[<$proto:lower>]::response::Msg::End(_)) => {
                            Ok(())
                        }
                        Some(usbsas_proto::[<$proto:lower>]::response::Msg::Error(e)) => {
                            Err(std::io::Error::new(std::io::ErrorKind::Other, e.err))
                        }
                        _ => Err(std::io::Error::new(std::io::ErrorKind::Other, "Unexpected response"))
                    }
                }
                fn recv_status(&mut self) -> std::io::Result<usbsas_proto::common::ResponseStatus> {
                    let resp: usbsas_proto::[<$proto:lower>]::Response = self.recv()?;
                    match resp.msg {
                        Some(usbsas_proto::[<$proto:lower>]::response::Msg::Status(status)) => Ok(status),
                        Some(usbsas_proto::[<$proto:lower>]::response::Msg::Error(err)) => Err(std::io::Error::new(std::io::ErrorKind::Other, err.err)),
                        _ => Err(std::io::Error::new(std::io::ErrorKind::Other, "Unexpected response"))
                    }
                }

            }
            impl<R: Read, W: Write> [<ProtoReq$proto>] for Comm<usbsas_proto::[<$proto:lower>]::Request, R, W> {
                $(
                    fn [<$req:lower>](&mut self, req: usbsas_proto::[<$proto:lower>]::[<Request$req>]) -> std::io::Result<usbsas_proto::[<$proto:lower>]::[<Response$req>]> {
                        let req_parts = usbsas_proto::[<$proto:lower>]::Request {
                            msg: Some(usbsas_proto::[<$proto:lower>]::request::Msg::$req(req)),
                        };
                        self.send(req_parts)?;
                        let resp: usbsas_proto::[<$proto:lower>]::Response = self.recv()?;
                        match resp.msg {
                            Some(usbsas_proto::[<$proto:lower>]::response::Msg::$req(info)) => {
                                Ok(info)
                            }
                            Some(usbsas_proto::[<$proto:lower>]::response::Msg::Error(e)) => {
                                Err(std::io::Error::new(std::io::ErrorKind::Other, e.err))
                            }
                            _ => Err(std::io::Error::new(std::io::ErrorKind::Other, "Unexpected response"))
                        }
                    }
                )+
            }
        }
    };
}

/// Same as protorequest but for responses
#[macro_export]
macro_rules! protoresponse {
    ($proto:ident,
     $($resp:ident),+) => {
        paste!{
            pub type [<ComRp$proto>] = Comm<usbsas_proto::[<$proto:lower>]::Response, File, File>;
            pub trait [<ProtoResp$proto>]: SendRecv + ProtoRespCommon {
                fn recv_req(&mut self) -> std::io::Result<usbsas_proto::[<$proto:lower>]::request::Msg>;
                $(
                    fn [<$resp:lower>](&mut self, resp: usbsas_proto::[<$proto:lower>]::[<Response$resp>]) -> std::io::Result<()>;
                )+
            }
            impl<R: Read, W: Write> ProtoRespCommon for Comm<usbsas_proto::[<$proto:lower>]::Response, R, W> {
                fn status(&mut self, current: u64, total: u64, done: bool, status: usbsas_proto::common::Status) -> std::io::Result<()> {
                    self.send(usbsas_proto::[<$proto:lower>]::Response {
                        msg: Some(usbsas_proto::[<$proto:lower>]::response::Msg::Status(usbsas_proto::common::ResponseStatus {
                            current, total, done
                        })),
                    })
                }
                fn error(&mut self, err: impl std::string::ToString) -> std::io::Result<()> {
                    self.send(usbsas_proto::[<$proto:lower>]::Response {
                        msg: Some(usbsas_proto::[<$proto:lower>]::response::Msg::Error(usbsas_proto::common::ResponseError { err: err.to_string() })),
                    })
                }
                fn end(&mut self) -> std::io::Result<()> {
                    self.send(usbsas_proto::[<$proto:lower>]::Response {
                        msg: Some(usbsas_proto::[<$proto:lower>]::response::Msg::End(usbsas_proto::common::ResponseEnd {})),
                    })
                }
                fn done(&mut self) -> std::io::Result<()> {
                    self.send(usbsas_proto::[<$proto:lower>]::Response {
                        msg: Some(usbsas_proto::[<$proto:lower>]::response::Msg::Status(usbsas_proto::common::ResponseStatus {
                            current: 0, total: 0, done: true,
                        })),
                    })
                }
            }
            impl<R: Read, W: Write> [<ProtoResp$proto>] for Comm<usbsas_proto::[<$proto:lower>]::Response, R, W> {
                fn recv_req(&mut self) -> std::io::Result<usbsas_proto::[<$proto:lower>]::request::Msg> {
                    let req: usbsas_proto::[<$proto:lower>]::Request = self.recv()?;
                    req.msg.ok_or(std::io::Error::new(std::io::ErrorKind::InvalidData, "Unhandled request"))
                }
                $(
                    fn [<$resp:lower>](&mut self, resp: usbsas_proto::[<$proto:lower>]::[<Response$resp>]) -> std::io::Result<()> {
                        self.send(usbsas_proto::[<$proto:lower>]::Response {
                            msg: Some(usbsas_proto::[<$proto:lower>]::response::Msg::$resp(resp)),
                        })
                    }
                )+
            }
        }
    };
}

protoresponse!(Analyzer, Analyze, Report);
protoresponse!(CmdExec, Exec, PostCopyExec);
protoresponse!(Downloader, Download, ArchiveInfos);
protoresponse!(
    Files,
    GetAttr,
    OpenDevice,
    OpenPartition,
    Partitions,
    ReadDir,
    ReadFile,
    ReadSectors
);
protoresponse!(Fs2Dev, DevSize, WriteFs, LoadBitVec, Wipe);
protoresponse!(Identificator, UserId);
protoresponse!(Scsi, OpenDevice, Partitions, ReadSectors);
protoresponse!(UsbDev, Devices);
protoresponse!(
    Usbsas,
    Devices,
    GetAttr,
    ImgDisk,
    InitTransfer,
    OpenDevice,
    OpenPartition,
    Partitions,
    ReadDir,
    SelectFiles,
    Report,
    UserId,
    Wipe
);
protoresponse!(Uploader, Upload);
protoresponse!(WriteDst, Init, NewFile, WriteFile, EndFile, WriteRaw, WriteData, Close, BitVec);

protorequest!(Analyzer, Analyze, Report);
protorequest!(CmdExec, Exec, PostCopyExec);
protorequest!(Downloader, Download, ArchiveInfos);
protorequest!(
    Files,
    GetAttr,
    OpenDevice,
    OpenPartition,
    Partitions,
    ReadDir,
    ReadFile,
    ReadSectors
);
protorequest!(Fs2Dev, DevSize, WriteFs, Wipe, LoadBitVec);
protorequest!(Identificator, UserId);
protorequest!(
    Usbsas,
    Devices,
    GetAttr,
    ImgDisk,
    InitTransfer,
    OpenDevice,
    OpenPartition,
    Partitions,
    ReadDir,
    SelectFiles,
    Report,
    UserId,
    Wipe
);
protorequest!(Scsi, OpenDevice, Partitions, ReadSectors);
protorequest!(UsbDev, Devices);
protorequest!(Uploader, Upload);
protorequest!(WriteDst, Init, NewFile, WriteFile, EndFile, Close, BitVec, WriteRaw, WriteData);
