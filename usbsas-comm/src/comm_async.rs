//! Async counterpart of [`Comm`](crate::Comm).
//!
//! Only useful for the client that needs to stay responsive while
//! communicating with usbsas.
//!
//! **Cancellation**: a request (send + recv) must be driven to completion. If
//! its future is dropped half-way, the stream is left desynchronized and the
//! next `recv` will decode garbage. These futures shouldn't be put in a
//! `select!` that may drop them.

use futures_lite::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use pastey::paste;
use std::{io, marker::PhantomData};
use usbsas_utils::MAX_PROTO_MESS_SIZE;

/// Typed struct containing async input (read) and output (write) streams.
/// Like `Comm`, it is marked with the type of protobuf messages it will send.
pub struct CommAsync<U, R, W> {
    input: R,
    output: W,
    // fn() -> U so that Send / Sync of CommAsync only depend on R and W
    req: PhantomData<fn() -> U>,
}

impl<U, R, W> CommAsync<U, R, W> {
    /// New channel reading from `input` and writing to `output`.
    pub fn new(input: R, output: W) -> Self {
        CommAsync {
            input,
            output,
            req: PhantomData,
        }
    }

    /// Input stream of the channel.
    pub fn input(&self) -> &R {
        &self.input
    }

    /// Output stream of the channel.
    pub fn output(&self) -> &W {
        &self.output
    }

    /// Consume the channel and return its streams.
    pub fn into_inner(self) -> (R, W) {
        (self.input, self.output)
    }
}

impl<U, R: AsyncRead + Unpin, W: AsyncWrite + Unpin> CommAsync<U, R, W> {
    /// Encode and send a message, prefixed with its length (`u64` LE). The
    /// whole frame is written at once, then flushed.
    pub async fn send<T: prost::Message>(&mut self, req: T) -> io::Result<()> {
        let len = req.encoded_len();
        let mut buf = Vec::with_capacity(8 + len);
        buf.extend_from_slice(&(len as u64).to_le_bytes());
        req.encode(&mut buf)?;
        self.output.write_all(&buf).await?;
        self.output.flush().await
    }

    /// Receive and decode a message. Returns an error if the announced length
    /// is greater than `MAX_PROTO_MESS_SIZE`.
    pub async fn recv<T>(&mut self) -> io::Result<T>
    where
        T: prost::Message + Default,
    {
        let mut len_buf = [0u8; 8];
        self.input.read_exact(&mut len_buf).await?;
        let len = u64::from_le_bytes(len_buf);

        if len > MAX_PROTO_MESS_SIZE {
            return Err(io::Error::other("Message size exceeded"));
        }

        let mut buf = vec![0; len as usize];
        self.input.read_exact(&mut buf).await?;
        Ok(prost::Message::decode(buf.as_slice())?)
    }
}

/// Async equivalent of `protorequest!`: implements `end()`, `recv_status()`
/// and one async method per request directly on
/// `CommAsync<usbsas_proto::<proto>::Request, R, W>`.
///
/// ```rust,ignore
/// let mut comm: ComRqAsyncUsbsas<_, _> = CommAsync::new(input, output);
/// let devices = comm.devices(RequestDevices { include_alt: true }).await?.devices;
/// ```
macro_rules! protorequest_async {
    ($proto:ident,
     $($req:ident),+) => {
        paste! {
            pub type [<ComRqAsync$proto>]<R, W> = CommAsync<usbsas_proto::[<$proto:lower>]::Request, R, W>;

            impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> CommAsync<usbsas_proto::[<$proto:lower>]::Request, R, W> {
                pub async fn end(&mut self) -> io::Result<()> {
                    self.send(usbsas_proto::[<$proto:lower>]::Request {
                        msg: Some(usbsas_proto::[<$proto:lower>]::request::Msg::End(usbsas_proto::[<$proto:lower>]::RequestEnd {})),
                    }).await?;
                    let resp: usbsas_proto::[<$proto:lower>]::Response = self.recv().await?;
                    match resp.msg {
                        Some(usbsas_proto::[<$proto:lower>]::response::Msg::End(_)) => Ok(()),
                        Some(usbsas_proto::[<$proto:lower>]::response::Msg::Error(e)) => Err(io::Error::other(e.err)),
                        _ => Err(io::Error::other("Unexpected response")),
                    }
                }

                pub async fn recv_status(&mut self) -> io::Result<usbsas_proto::common::ResponseStatus> {
                    let resp: usbsas_proto::[<$proto:lower>]::Response = self.recv().await?;
                    match resp.msg {
                        Some(usbsas_proto::[<$proto:lower>]::response::Msg::Status(status)) => Ok(status),
                        Some(usbsas_proto::[<$proto:lower>]::response::Msg::Error(err)) => Err(io::Error::other(err.err)),
                        _ => Err(io::Error::other("Unexpected response")),
                    }
                }

                $(
                    pub async fn [<$req:lower>](&mut self, req: usbsas_proto::[<$proto:lower>]::[<Request$req>]) -> io::Result<usbsas_proto::[<$proto:lower>]::[<Response$req>]> {
                        self.send(usbsas_proto::[<$proto:lower>]::Request {
                            msg: Some(usbsas_proto::[<$proto:lower>]::request::Msg::$req(req)),
                        }).await?;
                        let resp: usbsas_proto::[<$proto:lower>]::Response = self.recv().await?;
                        match resp.msg {
                            Some(usbsas_proto::[<$proto:lower>]::response::Msg::$req(info)) => Ok(info),
                            Some(usbsas_proto::[<$proto:lower>]::response::Msg::Error(e)) => Err(io::Error::other(e.err)),
                            _ => Err(io::Error::other("Unexpected response")),
                        }
                    }
                )+
            }
        }
    };
}

// Only the client talks to usbsas asynchronously.
protorequest_async!(
    Usbsas,
    Devices,
    GetAttr,
    ImgDisk,
    InitTransfer,
    OpenPartition,
    Partitions,
    ReadDir,
    SelectFiles,
    Report,
    UserId,
    Wipe
);

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future::block_on;
    use prost::Message;
    use usbsas_proto::{
        common::{ResponseEnd, ResponseError, ResponseStatus, Status},
        usbsas::{
            Request, RequestEnd, RequestReadDir, RequestUserId, Response, ResponseUserId, request,
            response,
        },
    };

    type MemCommAsync<'a> = ComRqAsyncUsbsas<&'a [u8], Vec<u8>>;

    fn frames(msgs: impl IntoIterator<Item = Response>) -> Vec<u8> {
        let mut buf = Vec::new();
        for msg in msgs {
            buf.extend_from_slice(&(msg.encoded_len() as u64).to_le_bytes());
            msg.encode(&mut buf).unwrap();
        }
        buf
    }

    fn resp(msg: response::Msg) -> Response {
        Response { msg: Some(msg) }
    }

    fn sent_requests(mut buf: &[u8]) -> Vec<Request> {
        let mut reqs = Vec::new();
        while !buf.is_empty() {
            let len = u64::from_le_bytes(buf[..8].try_into().unwrap()) as usize;
            reqs.push(Request::decode(&buf[8..8 + len]).unwrap());
            buf = &buf[8 + len..];
        }
        reqs
    }

    #[test]
    fn same_wire_format_as_comm() {
        let msg = RequestReadDir {
            path: "/some/path".into(),
        };
        let mut comm: CommAsync<(), &[u8], Vec<u8>> = CommAsync::new(&[], Vec::new());
        block_on(comm.send(msg.clone())).unwrap();
        let (_, out) = comm.into_inner();
        let mut expected = (msg.encoded_len() as u64).to_le_bytes().to_vec();
        expected.extend(msg.encode_to_vec());
        assert_eq!(out, expected);

        let mut comm: CommAsync<(), &[u8], Vec<u8>> = CommAsync::new(&out, Vec::new());
        assert_eq!(block_on(comm.recv::<RequestReadDir>()).unwrap(), msg);
        let err = block_on(comm.recv::<RequestReadDir>()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn recv_rejects_oversized_message() {
        let input = (MAX_PROTO_MESS_SIZE + 1).to_le_bytes();
        let mut comm: CommAsync<(), &[u8], Vec<u8>> = CommAsync::new(&input, Vec::new());
        let err = block_on(comm.recv::<Request>()).unwrap_err();
        assert_eq!(err.to_string(), "Message size exceeded");
    }

    #[test]
    fn recv_truncated_and_invalid() {
        let input = [1, 0, 0];
        let mut comm: CommAsync<(), &[u8], Vec<u8>> = CommAsync::new(&input, Vec::new());
        let err = block_on(comm.recv::<Request>()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);

        let input = [3, 0, 0, 0, 0, 0, 0, 0, 0x0a, 0x0a, 0x00];
        let mut comm: CommAsync<(), &[u8], Vec<u8>> = CommAsync::new(&input, Vec::new());
        let err = block_on(comm.recv::<RequestReadDir>()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn request_responses() {
        let input = frames([
            resp(response::Msg::UserId(ResponseUserId {
                userid: "alice".into(),
            })),
            resp(response::Msg::Error(ResponseError {
                err: "no such dir".into(),
            })),
            resp(response::Msg::UserId(ResponseUserId::default())),
            Response { msg: None },
        ]);
        let mut comm: MemCommAsync = CommAsync::new(&input, Vec::new());
        block_on(async {
            assert_eq!(comm.userid(RequestUserId {}).await.unwrap().userid, "alice");
            let err = comm
                .readdir(RequestReadDir { path: "/x".into() })
                .await
                .unwrap_err();
            assert_eq!(err.to_string(), "no such dir");
            let err = comm.readdir(RequestReadDir::default()).await.unwrap_err();
            assert_eq!(err.to_string(), "Unexpected response");
            let err = comm.userid(RequestUserId {}).await.unwrap_err();
            assert_eq!(err.to_string(), "Unexpected response");
        });
        let (_, out) = comm.into_inner();
        assert_eq!(
            sent_requests(&out),
            [
                Request {
                    msg: Some(request::Msg::UserId(RequestUserId {}))
                },
                Request {
                    msg: Some(request::Msg::ReadDir(RequestReadDir { path: "/x".into() }))
                },
                Request {
                    msg: Some(request::Msg::ReadDir(RequestReadDir::default()))
                },
                Request {
                    msg: Some(request::Msg::UserId(RequestUserId {}))
                },
            ]
        );
    }

    #[test]
    fn recv_status() {
        let status = ResponseStatus {
            current: 5,
            total: 10,
            done: false,
            status: Status::UploadAv.into(),
        };
        let input = frames([
            resp(response::Msg::Status(status)),
            resp(response::Msg::Error(ResponseError {
                err: "upload failed".into(),
            })),
            resp(response::Msg::End(ResponseEnd {})),
        ]);
        let mut comm: MemCommAsync = CommAsync::new(&input, Vec::new());
        block_on(async {
            assert_eq!(comm.recv_status().await.unwrap(), status);
            assert_eq!(
                comm.recv_status().await.unwrap_err().to_string(),
                "upload failed"
            );
            assert_eq!(
                comm.recv_status().await.unwrap_err().to_string(),
                "Unexpected response"
            );
        });
        assert!(comm.output().is_empty());
    }

    #[test]
    fn end() {
        let input = frames([
            resp(response::Msg::End(ResponseEnd {})),
            resp(response::Msg::Error(ResponseError { err: "busy".into() })),
        ]);
        let mut comm: MemCommAsync = CommAsync::new(&input, Vec::new());
        block_on(async {
            comm.end().await.unwrap();
            assert_eq!(comm.end().await.unwrap_err().to_string(), "busy");
        });
        assert_eq!(
            sent_requests(comm.output()),
            vec![
                Request {
                    msg: Some(request::Msg::End(RequestEnd {}))
                };
                2
            ]
        );
    }
}
