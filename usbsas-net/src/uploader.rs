use crate::FileReaderProgress;
use crate::{Error, HttpClient, Result};
use byteorder::ReadBytesExt;
use log::{error, trace};
use reqwest::blocking::Body;
use std::fs::File;
use usbsas_comm::{ComRpUploader, ProtoRespCommon, ProtoRespUploader, SendRecv};
use usbsas_proto as proto;
use usbsas_proto::uploader::request::Msg;

enum State {
    Init(InitState),
    Running(RunningState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpUploader) -> Result<Self> {
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
}

struct RunningState {
    file: Option<File>,
}

struct WaitEndState {}

impl InitState {
    fn run(mut self, comm: &mut ComRpUploader) -> Result<State> {
        let cleantarpath = format!("{}_clean.tar", self.tarpath.trim_end_matches(".tar"));
        usbsas_sandbox::landlock(
            Some(&[
                &self.tarpath,
                &cleantarpath,
                "/etc",
                "/lib",
                "/usr/lib/",
                "/var/lib/usbsas",
            ]),
            None,
        )?;

        match comm.read_u8()? {
            // Nothing to do, exit
            0 => return Ok(State::WaitEnd(WaitEndState {})),
            // Use provided tar path
            1 => (),
            // Files of this transfer were analyzed, use clean tar path
            2 => self.tarpath = cleantarpath,
            _ => {
                error!("bad unlock value");
                return Ok(State::WaitEnd(WaitEndState {}));
            }
        }

        let file = File::open(self.tarpath)?;

        Ok(State::Running(RunningState { file: Some(file) }))
    }
}

impl RunningState {
    fn run(mut self, comm: &mut ComRpUploader) -> Result<State> {
        let req: proto::uploader::Request = comm.recv()?;
        match req.msg.ok_or(Error::BadRequest)? {
            Msg::Upload(req) => {
                if let Err(err) = self.upload(comm, req) {
                    error!("upload error: {}", err);
                    comm.error(err)?;
                };
                Ok(State::WaitEnd(WaitEndState {}))
            }
            Msg::End(_) => {
                comm.end()?;
                Ok(State::End)
            }
        }
    }

    fn upload(
        &mut self,
        comm: &mut ComRpUploader,
        req: proto::uploader::RequestUpload,
    ) -> Result<()> {
        trace!("upload");
        let network = req.network.ok_or(Error::BadRequest)?;
        let url = format!("{}/{}", network.url.trim_end_matches('/'), req.id);
        let mut http_client = HttpClient::new(
            #[cfg(feature = "authkrb")]
            if !network.krb_service_name.is_empty() {
                Some(network.krb_service_name)
            } else {
                None
            },
        )?;
        let file = self
            .file
            .take()
            .ok_or_else(|| Error::Error("no file to upload".to_string()))?;
        let filesize = file.metadata()?.len();

        let comm_progress = comm.try_clone()?;

        let filereaderprogress = FileReaderProgress {
            comm: comm_progress,
            file,
            filesize,
            offset: 0,
        };

        let body = Body::sized(filereaderprogress, filesize);

        let resp = http_client.post(&url, body)?;
        if !resp.status().is_success() {
            return Err(Error::Upload(format!(
                "Unknown status code {:?}",
                resp.status()
            )));
        }

        comm.upload(proto::uploader::ResponseUpload {})?;
        Ok(())
    }
}

impl WaitEndState {
    fn run(self, comm: &mut ComRpUploader) -> Result<State> {
        trace!("wait end state");
        loop {
            let req: proto::uploader::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
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

pub struct Uploader {
    comm: ComRpUploader,
    state: State,
}

impl Uploader {
    pub fn new(comm: ComRpUploader, tarpath: String) -> Result<Self> {
        let state = State::Init(InitState { tarpath });
        Ok(Uploader { comm, state })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}, waiting end", err);
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}
