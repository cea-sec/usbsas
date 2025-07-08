use crate::{Error, FileReaderProgress, HttpClient, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use log::{error, trace};
use reqwest::blocking::Body;
use std::fs::File;
use usbsas_comm::{ComRpUploader, ProtoRespCommon, ProtoRespUploader};
use usbsas_proto as proto;
use usbsas_proto::{common::Status, uploader::request::Msg};

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
            Some(
                &[
                    crate::NET_PATHS_RO,
                    #[cfg(feature = "authkrb")]
                    crate::KRB5_PATHS_RO,
                    &[&self.tarpath, &cleantarpath],
                ]
                .concat(),
            ),
            None,
            None,
        )?;

        match comm.read_u64::<LittleEndian>()? {
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
        match comm.recv_req()? {
            Msg::Upload(req) => {
                if let Err(err) = self.upload(comm, req) {
                    error!("upload error: {err}");
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
        comm.upload(proto::uploader::ResponseUpload {})?;
        let network = req.network.ok_or(Error::BadRequest)?;
        let url = format!("{}/{}", network.url.trim_end_matches('/'), req.id);
        let mut http_client = HttpClient::new(
            #[cfg(feature = "authkrb")]
            network.krb_service_name,
        )?;
        let file = self
            .file
            .take()
            .ok_or_else(|| Error::Error("no file to upload".to_string()))?;
        let filesize = file.metadata()?.len();

        let comm_progress =
            ComRpUploader::new(comm.input().try_clone()?, comm.output().try_clone()?);

        let filereaderprogress = FileReaderProgress {
            comm: comm_progress,
            file,
            filesize,
            offset: 0,
            status: Status::UploadDst,
        };

        let body = Body::sized(filereaderprogress, filesize);

        let resp = http_client.post(&url, body)?;
        if !resp.status().is_success() {
            return Err(Error::Upload(format!(
                "Unknown status code {:?}",
                resp.status()
            )));
        }

        comm.done(Status::UploadDst)?;
        Ok(())
    }
}

impl WaitEndState {
    fn run(self, comm: &mut ComRpUploader) -> Result<State> {
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
                    error!("state run error: {err}, waiting end");
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            };
        }
        Ok(())
    }
}
