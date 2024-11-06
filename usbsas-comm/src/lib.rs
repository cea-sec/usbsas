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

pub trait ToFromFd {
    fn from_fd(read: OwnedFd, write: OwnedFd) -> Self;
    fn input_fd(&self) -> RawFd;
    fn output_fd(&self) -> RawFd;
}

impl<R> ToFromFd for Comm<R> {
    fn from_fd(read: OwnedFd, write: OwnedFd) -> Self {
        Comm {
            input: File::from(read),
            output: File::from(write),
            req: PhantomData,
        }
    }
    fn input_fd(&self) -> RawFd {
        self.input.as_raw_fd()
    }
    fn output_fd(&self) -> RawFd {
        self.output.as_raw_fd()
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

impl<R> SendRecv for Comm<R> {}

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
/// protorequest!(Usbdev, Devices, End);
///
/// let comm: CommUsbdev = Comm::from_env()?;
///
/// // Send RequestDevices and receive a ResponseDevices
/// let devices = comm.devices()?.devices;
///
/// ```

pub trait ProtoReqCommon: SendRecv {
    fn end(&mut self) -> std::io::Result<()>;
}

#[macro_export]
macro_rules! protorequest {
    ($proto:ident,
     $($req:ident),+) => {
        paste! {
            pub type [<ComRq$proto>] = Comm<usbsas_proto::[<$proto:lower>]::Request>;
            pub trait [<ProtoReq$proto>]: SendRecv + ProtoReqCommon {
                $(
                    fn [<$req:lower>](&mut self, req: usbsas_proto::[<$proto:lower>]::[<Request$req>]) -> std::io::Result<usbsas_proto::[<$proto:lower>]::[<Response$req>]>;
                )+
            }
            impl ProtoReqCommon for [<ComRq$proto>] {
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
            }
            impl [<ProtoReq$proto>] for [<ComRq$proto>] {
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

pub trait ProtoRespCommon: SendRecv {
    fn status(&mut self, current: u64, total: u64, done: bool) -> std::io::Result<()>;
    fn error(&mut self, err: impl std::string::ToString) -> std::io::Result<()>;
    fn end(&mut self) -> std::io::Result<()>;
    fn done(&mut self) -> std::io::Result<()>;
}

#[macro_export]
macro_rules! protoresponse {
    ($proto:ident,
     $($resp:ident),+) => {
        paste!{
            pub type [<ComRp$proto>] = Comm<usbsas_proto::[<$proto:lower>]::Response>;
            pub trait [<ProtoResp$proto>]: SendRecv + ProtoRespCommon {
                fn recv_req(&mut self) -> std::io::Result<usbsas_proto::[<$proto:lower>]::request::Msg>;
                $(
                    fn [<$resp:lower>](&mut self, resp: usbsas_proto::[<$proto:lower>]::[<Response$resp>]) -> std::io::Result<()>;
                )+
            }
            impl ProtoRespCommon for [<ComRp$proto>] {
                fn status(&mut self, current: u64, total: u64, done: bool) -> std::io::Result<()> {
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
            impl [<ProtoResp$proto>] for [<ComRp$proto>] {
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

protoresponse!(CmdExec, Exec, PostCopyExec);
protoresponse!(Fs2Dev, DevSize, StartCopy, LoadBitVec, Wipe);
protoresponse!(Scsi, OpenDevice, Partitions, ReadSectors);
protoresponse!(WriteFs, SetFsInfos, NewFile, WriteFile, EndFile, ImgDisk, WriteData, Close, BitVec);
protoresponse!(WriteTar, NewFile, WriteFile, EndFile, Close);
protoresponse!(Filter, FilterPaths);
protoresponse!(Identificator, Id);
protoresponse!(UsbDev, Devices);
protoresponse!(Analyzer, Analyze);
protoresponse!(Downloader, Download, ArchiveInfos);
protoresponse!(Uploader, Upload);
protoresponse!(
    Files,
    OpenDevice,
    Partitions,
    OpenPartition,
    GetAttr,
    ReadDir,
    ReadFile,
    ReadSectors
);
protorequest!(Scsi, OpenDevice, Partitions, ReadSectors);
protorequest!(
    Usbsas,
    Id,
    UsbDevices,
    AltTargets,
    OpenDevice,
    Partitions,
    OpenPartition,
    ReadDir,
    GetAttr,
    PostCopyCmd,
    Wipe,
    ImgDisk
);
protorequest!(Fs2Dev, DevSize, StartCopy, Wipe, LoadBitVec);
protorequest!(
    Files,
    OpenDevice,
    Partitions,
    OpenPartition,
    GetAttr,
    ReadDir,
    ReadFile,
    ReadSectors
);
protorequest!(WriteFs, SetFsInfos, NewFile, WriteFile, EndFile, Close, BitVec, ImgDisk, WriteData);
protorequest!(Uploader, Upload);
protorequest!(Downloader, Download, ArchiveInfos);
protorequest!(Analyzer, Analyze);

protoresponse!(
    Usbsas,
    Id,
    UsbDevices,
    AltTargets,
    OpenDevice,
    OpenPartition,
    Partitions,
    GetAttr,
    ReadDir,
    CopyStart,
    CopyDone,
    FinalCopyStatusDone,
    NotEnoughSpace,
    NothingToCopy,
    Wipe,
    ImgDisk,
    PostCopyCmd
);
protorequest!(Filter, FilterPaths);
protorequest!(Identificator, Id);
protorequest!(UsbDev, Devices);
protorequest!(WriteTar, NewFile, WriteFile, EndFile, Close);
protorequest!(CmdExec, Exec, PostCopyExec);
