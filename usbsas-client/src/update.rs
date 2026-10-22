//! Message handling: state transitions and requests to usbsas.

use crate::{ComRqUsbsas, GUI, Message, Response, SrcOpened, State, Status};
use iced::{
    Task,
    futures::{Stream, stream},
    window::{self, Mode},
};
use smol::lock::Mutex;
use std::{fs, future::Future, path, sync::Arc};
use usbsas_proto::{self as proto, common::device::Device};

/// Unwrap a result or switch to `State::Error` and return from the handler.
macro_rules! ok_or_err {
    ($s: ident, $e: expr) => {
        match $e {
            Ok(res) => res,
            Err(err) => {
                log::error!("{err}");
                $s.state = State::Error(format!("{err}"));
                return Task::none();
            }
        }
    };
}

fn err_str(err: std::io::Error) -> String {
    format!("{err}")
}

/// Stream status responses from usbsas until AllDone or an error. The comm is
/// locked for each status only.
fn recv_status(comm: Arc<Mutex<ComRqUsbsas>>) -> impl Stream<Item = Status> {
    stream::unfold(Some(comm), |comm| async move {
        let comm = comm?;
        let res = comm.lock().await.recv_status().await;
        let (status, done) = match res {
            Ok(resp) => {
                let done = matches!(
                    proto::common::Status::try_from(resp.status),
                    Ok(proto::common::Status::AllDone)
                );
                (Status::Progress(resp), done)
            }
            Err(err) => (Status::Error(format!("{err}")), true),
        };
        Some((status, if done { None } else { Some(comm) }))
    })
}

/// Request the id of the user from usbsas.
async fn req_userid(comm: Arc<Mutex<ComRqUsbsas>>) -> Response {
    let res = comm
        .lock()
        .await
        .userid(proto::usbsas::RequestUserId {})
        .await;
    Response::UserId(res.map(|resp| resp.userid).map_err(err_str))
}

impl GUI {
    /// Remove `path` from the selection, and its children if
    /// `unselect_children`. Its parent directories are unselected too since
    /// they are no longer fully selected.
    fn unselect_path(&mut self, path: &str, unselect_children: bool) {
        self.selected.remove(path);
        if unselect_children {
            self.selected
                .retain(|x| !x.starts_with(&format!("{}/", path)));
        }
        // remove parent(s)
        if let Some((parent, _)) = path.rsplit_once('/') {
            if !parent.is_empty() {
                self.unselect_path(parent, false);
            } else {
                self.selected.remove("/");
            }
        };
    }

    /// Send a request to usbsas without blocking the GUI. The future built by
    /// `req` runs on iced's executor and its result comes back as
    /// `Message::Response`, tagged with the current generation.
    ///
    /// `polling` requests (devices refresh) don't block user actions, other
    /// requests set `busy` until their response is handled.
    fn request<F>(
        &mut self,
        polling: bool,
        req: impl FnOnce(Arc<Mutex<ComRqUsbsas>>) -> F,
    ) -> Task<Message>
    where
        F: Future<Output = Response> + Send + 'static,
    {
        let Some(comm) = self.comm.clone() else {
            log::error!("not connected");
            self.state = State::Error("not connected".into());
            return Task::none();
        };
        if polling {
            self.polling = true;
        } else {
            self.busy = true;
        }
        let generation = self.generation;
        Task::perform(req(comm), move |resp| Message::Response(generation, resp))
    }

    /// Forward status updates of the current operation as `Message::Status`.
    fn status_stream(&self) -> Task<Message> {
        let generation = self.generation;
        match &self.comm {
            Some(comm) => Task::stream(recv_status(comm.clone()))
                .map(move |status| Message::Status(generation, status)),
            None => Task::none(),
        }
    }

    /// Handle the response of a request sent with `self.request()`
    fn response(&mut self, generation: u64, resp: Response) -> Task<Message> {
        if generation != self.generation {
            log::debug!("dropping response from previous session: {resp:?}");
            return Task::none();
        }
        if matches!(resp, Response::Devices(_)) {
            self.polling = false;
        } else {
            self.busy = false;
        }

        match resp {
            Response::Devices(res) => {
                self.devices = crate::devices_from_proto(ok_or_err!(self, res));
                let usb_count = self
                    .devices
                    .iter()
                    .filter(|(_, dev)| matches!(dev, Device::Usb(_)))
                    .count();
                match self.state {
                    State::Init if usb_count > 0 => self.state = State::DevSelect,
                    State::DevSelect if usb_count == 0 => self.state = State::Init,
                    _ => (),
                }
            }
            Response::UserId(res) => match res {
                Ok(userid) => {
                    self.userid = Some(userid);
                    match self.state {
                        State::UserID => {
                            self.state = State::DevSelect;
                            return Task::done(Message::Ok);
                        }
                        // userid was requested by Message::Ok, resume it
                        State::DevSelect | State::DownloadPin => {
                            return Task::done(Message::Ok);
                        }
                        _ => (),
                    }
                }
                Err(err) => {
                    log::debug!("no user id: {err}");
                    if matches!(self.state, State::DevSelect | State::DownloadPin) {
                        self.state = State::UserID;
                    }
                }
            },
            Response::InitTransfer(res) => match ok_or_err!(self, res) {
                SrcOpened::Usb(partitions) => {
                    if partitions.len() == 1 {
                        return Task::done(Message::PartSelect(0));
                    } else {
                        self.state = State::PartSelect(partitions);
                    }
                }
                SrcOpened::Network => {
                    self.state = State::Status(Status::init());
                    return self.status_stream();
                }
                SrcOpened::LocalDir => {
                    return Task::done(Message::ReadDir("/".into()));
                }
                SrcOpened::Other => (),
            },
            Response::OpenPartition(res) => {
                ok_or_err!(self, res);
                return Task::done(Message::ReadDir("/".into()));
            }
            Response::ReadDir(path, res) => {
                let filesinfo = ok_or_err!(self, res);
                self.current_dir = path;
                self.current_files = Vec::new();
                filesinfo
                    .iter()
                    .for_each(|x| self.current_files.push(x.path.clone()));
                // if parent selected, select children
                if self.selected.contains(&self.current_dir) {
                    filesinfo.iter().for_each(|x| {
                        self.selected.insert(x.path.clone());
                    });
                }
                // if all children selected, select parents
                if !self.current_files.is_empty()
                    && self.current_files.iter().all(|x| self.selected.contains(x))
                {
                    self.selected.insert(self.current_dir.clone());
                }
                self.state = State::ReadDir(filesinfo);
            }
            Response::StartStatus(res) => {
                ok_or_err!(self, res);
                self.state = State::Status(Status::init());
                return self.status_stream();
            }
            Response::Report(res) => {
                self.report = ok_or_err!(self, res);
                if let Some(ref report) = self.report {
                    log::debug!("{:#?}", report);
                    if let Some(ref report_conf) = self.config.report
                        && let Some(path) = &report_conf.write_local
                    {
                        let filename = format!(
                            "usbsas_report_{}_{}.json",
                            report.datetime, report.transfer_id
                        );
                        match fs::File::create(path::Path::new(path).join(filename)) {
                            Ok(mut file) => {
                                if let Err(err) = serde_json::to_writer_pretty(&mut file, report) {
                                    log::error!("Error writing report: {err}");
                                };
                            }
                            Err(err) => {
                                log::error!("couldn't write report: {err}");
                            }
                        }
                    };
                };
                self.state = State::Done;
            }
        }
        Task::none()
    }

    /// iced update function, see [`Message`] for the meaning of each message.
    ///
    /// When the client is in fullscreen mode, the window is also put back in
    /// fullscreen if needed.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        if let Message::Status(generation, _) = message
            && (generation != self.generation || !matches!(self.state, State::Status(_)))
        {
            return Task::none();
        };
        // Messages that may send a request: ignore them while another
        // user-triggered request is in flight (e.g. double click on Ok).
        if self.busy
            && matches!(
                message,
                Message::Ok
                    | Message::Devices
                    | Message::UserID
                    | Message::InitTransfer
                    | Message::PartSelect(_)
                    | Message::ReadDir(_)
                    | Message::PreviousDir
                    | Message::ListFiles
            )
        {
            log::debug!("request in flight, ignoring {message:?}");
            return Task::none();
        }
        match message {
            Message::Faq => self.state = State::Faq,
            Message::Tools => self.state = State::Tools,
            Message::SysInfo => self.state = State::SysInfo,
            Message::LangSelect(lang) => self.lang = lang,
            Message::FsTypeSelect(fstype) => self.fstype = fstype,
            Message::Devices => {
                if !self.polling {
                    return self.request(true, |comm| async move {
                        let res = comm
                            .lock()
                            .await
                            .devices(proto::usbsas::RequestDevices { include_alt: true })
                            .await;
                        Response::Devices(res.map(|resp| resp.devices).map_err(err_str))
                    });
                }
            }
            Message::UserID => return self.request(false, req_userid),
            Message::UserInput(input) => {
                if let Some(ref mut cur) = self.download_pin {
                    cur.push(char::from(input + 48));
                } else {
                    self.download_pin = Some(String::from(char::from(input + 48)));
                };
            }
            Message::ClearInput(all) => {
                if all {
                    self.download_pin = None;
                } else if let Some(ref mut cur) = self.download_pin {
                    let _ = cur.pop();
                };
            }
            Message::Ok => match self.state {
                State::DevSelect | State::DownloadPin => {
                    if self.userid.is_none() {
                        // Message::Ok is sent again once the userid is received
                        return self.request(false, req_userid);
                    }
                    if let (Some(src_id), Some(_)) = (self.src_id, self.dst_id) {
                        if let Some(Device::Network(_)) = self.devices.get(&src_id)
                            && self.download_pin.is_none()
                        {
                            self.state = State::DownloadPin;
                            return Task::none();
                        };
                        self.state = State::Wait("wait_open_dev".to_string());
                        return Task::done(Message::InitTransfer);
                    };
                }
                State::ReadDir(_) => {
                    self.state = State::Wait("wait_list_files".to_string());
                    return Task::done(Message::ListFiles);
                }
                State::Wipe(quick) => {
                    if let Some(dst_id) = self.dst_id {
                        self.status_title = Some("wipe_title".into());
                        let req = proto::usbsas::RequestWipe {
                            id: dst_id,
                            quick,
                            fstype: self.fstype.into(),
                        };
                        self.state = State::Status(Status::init());
                        return self.request(false, |comm| async move {
                            let res = comm.lock().await.wipe(req).await;
                            Response::StartStatus(res.map(|_| ()).map_err(err_str))
                        });
                    }
                }
                State::DiskImg => {
                    if let Some(src_id) = self.src_id {
                        self.status_title = Some("diskimg".into());
                        self.state = State::Status(Status::init());
                        return self.request(false, move |comm| async move {
                            let res = comm
                                .lock()
                                .await
                                .imgdisk(proto::usbsas::RequestImgDisk { id: src_id })
                                .await;
                            Response::StartStatus(res.map(|_| ()).map_err(err_str))
                        });
                    }
                }
                State::Done => {
                    self.state = State::Reload;
                    return Task::done(Message::Reset);
                }
                _ => (),
            },
            Message::Nok => match self.state {
                State::Tools | State::Wipe(_) | State::DiskImg | State::Faq | State::UserID => {
                    self.soft_reset();
                    self.state = State::Init;
                }
                State::DevSelect => {
                    self.soft_reset();
                }
                // Reset already in progress
                State::Reload => (),
                _ => {
                    self.state = State::Reload;
                    return Task::done(Message::Reset);
                }
            },
            Message::InitTransfer => {
                let (Some(src_id), Some(dst_id)) = (self.src_id, self.dst_id) else {
                    return Task::none();
                };
                let req = proto::usbsas::RequestInitTransfer {
                    source: src_id,
                    destination: dst_id,
                    fstype: Some(self.fstype.into()),
                    pin: self.download_pin.clone(),
                };
                let src = self.devices.get(&src_id).cloned();
                return self.request(false, move |comm| async move {
                    let mut comm = comm.lock().await;
                    let res = async {
                        comm.inittransfer(req).await?;
                        Ok::<_, std::io::Error>(match src {
                            Some(Device::Usb(_)) => SrcOpened::Usb(
                                comm.partitions(proto::usbsas::RequestPartitions {})
                                    .await?
                                    .partitions,
                            ),
                            Some(Device::Network(_)) => SrcOpened::Network,
                            Some(Device::LocalDir(_)) => SrcOpened::LocalDir,
                            _ => SrcOpened::Other,
                        })
                    }
                    .await;
                    Response::InitTransfer(res.map_err(err_str))
                });
            }
            Message::ListFiles => {
                let req = proto::usbsas::RequestSelectFiles {
                    selected: self.selected.clone().into_iter().collect(),
                };
                return self.request(false, |comm| async move {
                    let res = comm.lock().await.selectfiles(req).await;
                    Response::StartStatus(res.map(|_| ()).map_err(err_str))
                });
            }
            Message::Wipe(quick) => {
                self.state = State::Wipe(quick);
            }
            Message::DiskImg => {
                self.state = State::DiskImg;
            }
            Message::SrcSelect(new_src_id) => {
                self.src_id = Some(new_src_id);
                // Network to network transfer unsupported
                if let Some(dst_id) = self.dst_id
                    && (new_src_id == dst_id
                        || (matches!(self.devices.get(&new_src_id), Some(Device::Network(_)))
                            && matches!(self.devices.get(&dst_id), Some(Device::Network(_)))))
                {
                    self.dst_id = None;
                };
            }
            Message::DstSelect(new_dst_id) => {
                self.dst_id = Some(new_dst_id);
                if let Some(src_id) = self.src_id
                    && (new_dst_id == src_id
                        || (matches!(self.devices.get(&new_dst_id), Some(Device::Network(_)))
                            && matches!(self.devices.get(&src_id), Some(Device::Network(_)))))
                {
                    self.src_id = None;
                };
            }
            Message::PartSelect(index) => {
                return self.request(false, move |comm| async move {
                    let res = comm
                        .lock()
                        .await
                        .openpartition(proto::usbsas::RequestOpenPartition { index })
                        .await;
                    Response::OpenPartition(res.map(|_| ()).map_err(err_str))
                });
            }
            Message::ReadDir(path) => {
                // self.current_dir is updated when the response is received
                return self.request(false, |comm| async move {
                    let res = comm
                        .lock()
                        .await
                        .readdir(proto::usbsas::RequestReadDir { path: path.clone() })
                        .await;
                    Response::ReadDir(path, res.map(|resp| resp.filesinfo).map_err(err_str))
                });
            }
            Message::PreviousDir => {
                let parent = match self.current_dir.rfind('/') {
                    Some(0) | None => "/".to_string(),
                    Some(index) => self.current_dir[..index].to_string(),
                };
                return Task::done(Message::ReadDir(parent));
            }
            Message::SelectFile(path) => {
                self.selected.insert(path);
                if !self.current_files.is_empty()
                    && self.current_files.iter().all(|x| self.selected.contains(x))
                {
                    self.selected.insert(self.current_dir.clone());
                }
            }
            Message::UnSelectFile(path) => {
                self.unselect_path(&path, true);
            }
            Message::SelectAll(mut files) => {
                while let Some(file) = files.pop() {
                    let _ = self.selected.insert(file);
                }
                self.selected.insert(self.current_dir.clone());
            }
            Message::UnselectAll(paths) => {
                paths.iter().for_each(|path| {
                    self.unselect_path(path, true);
                });
                self.selected.remove(&self.current_dir);
            }
            Message::Status(_, status) => {
                if let Status::Progress(status) = status {
                    self.seen_status.insert(
                        status
                            .status
                            .try_into()
                            .unwrap_or(proto::common::Status::Unknown),
                    );
                };
                self.state = State::Status(status.clone());
                if let Status::Progress(status) = &status
                    && let Ok(proto::common::Status::AllDone) = &status.status.try_into()
                {
                    return self.request(false, |comm| async move {
                        let res = comm
                            .lock()
                            .await
                            .report(proto::usbsas::RequestReport {})
                            .await;
                        Response::Report(res.map(|resp| resp.report).map_err(err_str))
                    });
                }
            }
            Message::Tick(_) => match self.state {
                State::Sandbox => self.sandbox(),
                State::Connect => self.try_connect(),
                State::Init | State::DevSelect | State::Wipe(_) | State::DiskImg | State::Done => {
                    // Don't pile up requests if usbsas is slow to answer
                    if !self.polling && !self.busy {
                        return Task::done(Message::Devices);
                    }
                }
                State::UserID if !self.busy => {
                    return Task::done(Message::UserID);
                }
                _ => (),
            },
            Message::Reset => return self.reset(),
            Message::ResetDone(generation) => return self.reset_done(generation),
            Message::Response(generation, resp) => return self.response(generation, resp),
        }

        if self.fullscreen {
            window::latest().and_then(move |window| {
                window::mode(window).then(move |mode| {
                    if mode != Mode::Fullscreen {
                        window::set_mode(window, Mode::Fullscreen)
                    } else {
                        Task::none()
                    }
                })
            })
        } else {
            Task::none()
        }
    }
}
