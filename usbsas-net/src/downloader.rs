//! Downloader process: downloads an archive from the source network into the
//! session tar.
//!
//! States: `Init` (read configuration, sandbox) -> `Running` (`ArchiveInfos`,
//! `Download`) -> `WaitEnd` -> `End`.

use crate::FileWriterProgress;
use crate::{Error, HttpClient, Result};
use flate2::read::MultiGzDecoder;
use log::{error, trace};
use reqwest::{Url, header::CONTENT_ENCODING};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
};
use usbsas_comm::{ComRpDownloader, ProtoRespCommon, ProtoRespDownloader};
use usbsas_config::{conf_parse, conf_read};
use usbsas_proto as proto;
use usbsas_proto::{common::Status, downloader::request::Msg};

/// States of the process, see the module documentation.
enum State {
    Init(InitState),
    Running(RunningState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpDownloader) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::Running(s) => s.run(comm),
            State::WaitEnd(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {
    tarpath: String,
    config_path: String,
}

struct RunningState {
    file: File,
    url: String,
    http_client: HttpClient,
}

struct WaitEndState {}

impl InitState {
    fn run(self, _comm: &mut ComRpDownloader) -> Result<State> {
        let config = conf_parse(&conf_read(&self.config_path)?)?;
        let net_conf = config.source_network.ok_or(Error::NoConf)?;
        crate::check_url(&net_conf.url)?;
        let port = Url::parse(&net_conf.url)?
            .port_or_known_default()
            .ok_or(url::ParseError::InvalidPort)?;

        usbsas_sandbox::net::sandbox(
            Some(
                &[
                    crate::NET_PATHS_RO,
                    #[cfg(feature = "authkrb")]
                    crate::KRB5_PATHS_RO,
                    &[&self.config_path],
                ]
                .concat(),
            ),
            Some(&[&self.tarpath]),
            None,
            Some(&[
                port,
                #[cfg(feature = "authkrb")]
                crate::KRB_AS_PORT,
            ]),
        )?;

        let file = OpenOptions::new()
            .write(true)
            .read(false)
            .open(&self.tarpath)?;

        Ok(State::Running(RunningState {
            file,
            url: net_conf.url,
            http_client: HttpClient::new(
                #[cfg(feature = "authkrb")]
                net_conf.krb_service_name,
            )?,
        }))
    }
}

impl RunningState {
    fn run(mut self, comm: &mut ComRpDownloader) -> Result<State> {
        let mut filesize = None;
        loop {
            match comm.recv_req()? {
                Msg::ArchiveInfos(req) => {
                    match self.archive_infos(comm, &req.path) {
                        Ok(size) => filesize = Some(size),
                        Err(err) => {
                            error!("download error: {err}");
                            comm.error(err)?;
                        }
                    };
                }
                Msg::Download(_) => {
                    if let Some(size) = filesize {
                        if let Err(err) = self.download(comm, size) {
                            error!("download error: {err}");
                            comm.error(err)?;
                        };
                        return Ok(State::WaitEnd(WaitEndState {}));
                    } else {
                        comm.error("can't download before knowing the size")?;
                    }
                }
                Msg::End(_) => {
                    comm.end()?;
                    return Ok(State::End);
                }
            }
        }
    }

    /// HEAD `<url>/<path>`, the uncompressed size of the archive is in the
    /// `X-Uncompressed-Content-Length` header.
    fn archive_infos(&mut self, comm: &mut ComRpDownloader, path: &str) -> Result<u64> {
        trace!("req size");
        self.url = format!("{}/{}", self.url.trim_end_matches('/'), path);

        let resp = self.http_client.head(&self.url)?;
        if !resp.status().is_success() {
            return Err(Error::Error(format!(
                "Unknown status code {:?}",
                resp.status()
            )));
        }

        // Even if the archive is gzipped, we're expecting its uncompressed size here
        let size = resp
            .headers()
            .get("X-Uncompressed-Content-Length")
            .ok_or(Error::BadResponse)?
            .to_str()
            .map_err(|_| Error::BadResponse)?
            .parse::<u64>()
            .map_err(|_| Error::BadResponse)?;

        trace!("files size: {size}");

        comm.archiveinfos(proto::downloader::ResponseArchiveInfos { size })?;

        Ok(size)
    }

    /// GET the archive found with `archive_infos()` into the tar.
    fn download(mut self, comm: &mut ComRpDownloader, filesize: u64) -> Result<()> {
        trace!("download");
        comm.download(proto::downloader::ResponseDownload {})?;
        let comm_progress =
            ComRpDownloader::new(comm.input().try_clone()?, comm.output().try_clone()?);
        // The archive may be gzipped, `filesize` is its uncompressed size
        let max_wire_size = max_gzip_size(filesize);
        let resp = self.http_client.get_large(&self.url, max_wire_size)?;
        if !resp.status().is_success() {
            return Err(Error::Error(format!(
                "Unknown status code {:?}",
                resp.status()
            )));
        }

        let gzip = match resp
            .headers()
            .get(CONTENT_ENCODING)
            .map(|enc| enc.to_str().map(str::trim))
            .transpose()?
        {
            None => false,
            Some(enc) if enc.eq_ignore_ascii_case("identity") => false,
            Some(enc) if enc.eq_ignore_ascii_case("gzip") || enc.eq_ignore_ascii_case("x-gzip") => {
                true
            }
            Some(enc) => return Err(Error::ContentEncoding(enc.to_string())),
        };

        // Stop the transfer as soon as more than expected is received, on the
        // wire and after decompression.
        let body: Box<dyn Read> = if gzip {
            if let Some(len) = resp.content_length()
                && len > max_wire_size
            {
                return Err(Error::ResponseTooLarge(max_wire_size));
            }
            Box::new(MultiGzDecoder::new(LimitedReader::new(resp, max_wire_size)))
        } else {
            if let Some(len) = resp.content_length()
                && len != filesize
            {
                return Err(Error::DownloadSize {
                    received: len,
                    expected: filesize,
                });
            }
            Box::new(resp)
        };
        let mut body = LimitedReader::new(body, filesize);

        let mut filewriterprogress = FileWriterProgress {
            comm: comm_progress,
            file: self.file,
            filesize,
            offset: 0,
            status: Status::DlSrc,
        };

        let received = io::copy(&mut body, &mut filewriterprogress)?;
        if received != filesize {
            return Err(Error::DownloadSize {
                received,
                expected: filesize,
            });
        }
        comm.done(Status::DlSrc)?;
        Ok(())
    }
}

/// Maximum size of a gzip stream decompressing to `size` bytes. Worst case
/// for incompressible data is about 5 bytes per 64 KiB (stored blocks), plus
/// headers and trailers of each gzip member, this leaves a large margin.
fn max_gzip_size(size: u64) -> u64 {
    size.saturating_add(size / 1024).saturating_add(64 * 1024)
}

/// Reader failing as soon as more than `limit` bytes are read.
struct LimitedReader<R> {
    inner: R,
    limit: u64,
    read: u64,
}

impl<R: Read> LimitedReader<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            limit,
            read: 0,
        }
    }
}

impl<R: Read> Read for LimitedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Never read more than one byte past the limit
        let max = usize::try_from(self.limit - self.read + 1).unwrap_or(usize::MAX);
        let len = buf.len().min(max);
        let size_read = self.inner.read(&mut buf[..len])?;
        self.read += size_read as u64;
        if self.read > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("received more than {} bytes", self.limit),
            ));
        }
        Ok(size_read)
    }
}

impl WaitEndState {
    fn run(self, comm: &mut ComRpDownloader) -> Result<State> {
        trace!("wait end state");
        loop {
            match comm.recv_req()? {
                Msg::End(_) => {
                    comm.end()?;
                    break;
                }
                _ => {
                    error!("bad request");
                    comm.error("bad request")?;
                }
            }
        }
        Ok(State::End)
    }
}

/// Downloader process.
pub struct Downloader {
    comm: ComRpDownloader,
    state: State,
}

impl Downloader {
    /// New downloader writing to the tar at `tarpath`.
    pub fn new(comm: ComRpDownloader, tarpath: String, config_path: String) -> Result<Self> {
        let state = State::Init(InitState {
            tarpath,
            config_path,
        });
        Ok(Downloader { comm, state })
    }

    /// Run the process until it receives `End`. On error, the error is sent to
    /// the parent and the process waits for `End`.
    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(Error::NoConf) => {
                    log::warn!("No configuration for downloader, parking");
                    State::WaitEnd(WaitEndState {})
                }
                Err(err) => {
                    error!("state run error: {err}, waiting end");
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;

    fn read_all<R: Read>(mut reader: R) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        reader.read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn limited_reader() {
        let data = vec![1u8; 1000];
        assert_eq!(
            read_all(LimitedReader::new(&data[..], 1000)).unwrap().len(),
            1000
        );
        assert_eq!(
            read_all(LimitedReader::new(&data[..], 999))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            read_all(LimitedReader::new(&data[..0], 0)).unwrap().len(),
            0
        );
    }

    #[test]
    fn gzip_size_bound() {
        // Incompressible data, in several gzip members
        let mut state = 0x1234_5678_u32;
        let data: Vec<u8> = (0..4 * 1024 * 1024)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let mut gz = Vec::new();
        for chunk in data.chunks(1024 * 1024) {
            let mut enc = GzEncoder::new(Vec::new(), Compression::best());
            enc.write_all(chunk).unwrap();
            gz.extend(enc.finish().unwrap());
        }
        let max = max_gzip_size(data.len() as u64);
        assert!(gz.len() as u64 > data.len() as u64);
        assert!(gz.len() as u64 <= max);
        let out = read_all(LimitedReader::new(
            MultiGzDecoder::new(LimitedReader::new(&gz[..], max)),
            data.len() as u64,
        ))
        .unwrap();
        assert_eq!(out, data);
        // Decompresses to more than expected
        assert!(
            read_all(LimitedReader::new(
                MultiGzDecoder::new(&gz[..]),
                data.len() as u64 - 1
            ))
            .is_err()
        );
    }
}
