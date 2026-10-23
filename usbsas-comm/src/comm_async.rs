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
