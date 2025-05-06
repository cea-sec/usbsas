use crate::{Message, State, Status, GUI};
use iced::window::{self, Mode};
use std::{fs, path};
use usbsas_comm::ProtoReqUsbsas;
use usbsas_proto::{self as proto, common::device::Device};

macro_rules! ok_or_err {
    ($s: ident, $e: expr) => {
        match $e {
            Ok(res) => res,
            Err(err) => {
                log::error!("{err}");
                $s.state = State::Error(format!("{err}"));
                return iced::Task::none();
            }
        }
    };
}

macro_rules! comm_req {
    ($s: ident, $req: ident, $arg: expr) => {
        if let Some(comm) = &$s.comm {
            match comm.lock() {
                Ok(mut guard) => guard.$req($arg),
                Err(err) => {
                    log::error!("{err}");
                    $s.state = State::Error(format!("comm error: {err}"));
                    return iced::Task::none();
                }
            }
        } else {
            log::error!("not connected");
            $s.state = State::Error("not connected".into());
            return iced::Task::none();
        }
    };
}

impl GUI {
    pub fn update(&mut self, message: Message) -> iced::Task<Message> {
        macro_rules! comm {
            ($req: ident, $arg: expr) => {
                ok_or_err!(self, comm_req!(self, $req, $arg))
            };
        }
        match message {
            Message::Init => {
                self.try_connect();
            }
            Message::Faq => self.state = State::Faq,
            Message::Tools => self.state = State::Tools,
            Message::LangSelect(lang) => self.lang = lang,
            Message::FsTypeSelect(fstype) => self.fstype = fstype,
            Message::Devices => {
                self.devices = crate::devices_from_proto(
                    comm!(devices, proto::usbsas::RequestDevices { include_alt: true }).devices,
                );
                let usb_count = self
                    .devices
                    .iter()
                    .filter(|(_, dev)| matches!(dev, Device::Usb(_)))
                    .count();
                match self.state {
                    State::Init => {
                        if usb_count > 0 {
                            self.state = State::DevSelect
                        }
                    }
                    State::DevSelect => {
                        if usb_count == 0 {
                            self.state = State::Init
                        }
                    }
                    _ => (),
                }
            }
            Message::UserID => {
                let ret = comm_req!(self, userid, proto::usbsas::RequestUserId {});
                if let Ok(resp) = ret {
                    self.userid = Some(resp.userid);
                    self.state = State::DevSelect;
                    return self.update(Message::Ok);
                } else {
                    return iced::Task::none();
                };
            }
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
                        if let Ok(resp) = comm_req!(self, userid, proto::usbsas::RequestUserId {}) {
                            self.userid = Some(resp.userid);
                        } else {
                            self.state = State::UserID;
                            return iced::Task::none();
                        }
                    }
                    if let (Some(src_id), Some(dst_id)) = (self.src_id, self.dst_id) {
                        if let Some(Device::Network(_)) = self.devices.get(&src_id) {
                            if self.download_pin.is_none() {
                                self.state = State::DownloadPin;
                                return iced::Task::none();
                            }
                        };
                        comm!(
                            inittransfer,
                            proto::usbsas::RequestInitTransfer {
                                source: src_id,
                                destination: dst_id,
                                fstype: Some(self.fstype.into()),
                                pin: self.download_pin.clone(),
                            }
                        );
                        if let Some(Device::Usb(_)) = self.devices.get(&src_id) {
                            let partitions =
                                comm!(partitions, proto::usbsas::RequestPartitions {}).partitions;

                            if partitions.len() == 1 {
                                return self.update(Message::PartSelect(0));
                            } else {
                                self.state = State::PartSelect(partitions);
                            }
                        };
                        if let Some(Device::Network(_)) = self.devices.get(&src_id) {
                            self.state = State::Status(Status::init());
                        };
                    };
                }
                State::ReadDir(_) => {
                    comm!(
                        selectfiles,
                        proto::usbsas::RequestSelectFiles {
                            selected: self.selected.clone().into_iter().collect(),
                        }
                    );
                    self.state = State::Status(Status::init());
                }
                State::Wipe(quick) => {
                    if let Some(dst_id) = self.dst_id {
                        self.status_title = Some("wipe_title".into());
                        comm!(
                            wipe,
                            proto::usbsas::RequestWipe {
                                id: dst_id,
                                quick,
                                fstype: self.fstype.into(),
                            }
                        );
                        self.state = State::Status(Status::init());
                    }
                }
                State::DiskImg => {
                    if let Some(src_id) = self.src_id {
                        comm!(imgdisk, proto::usbsas::RequestImgDisk { id: src_id });
                        self.status_title = Some("diskimg".into());
                        self.state = State::Status(Status::init());
                    }
                }
                State::Done => {
                    if self
                        .devices
                        .iter()
                        .filter(|(id, dev)| {
                            matches!(dev, Device::Usb(_))
                                && (self.src_id == Some(*(*id)) || self.dst_id == Some(*(*id)))
                        })
                        .count()
                        == 0
                    {
                        self.state = State::Reload;
                    };
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
                _ => {
                    self.state = State::Reload;
                }
            },
            Message::Wipe(quick) => {
                self.state = State::Wipe(quick);
            }
            Message::DiskImg => {
                self.state = State::DiskImg;
            }
            Message::SrcSelect(new_src_id) => {
                self.src_id = Some(new_src_id);
                // Network to network transfer unsupported
                if let Some(dst_id) = self.dst_id {
                    if new_src_id == dst_id
                        || (matches!(self.devices.get(&new_src_id), Some(Device::Network(_)))
                            && matches!(self.devices.get(&dst_id), Some(Device::Network(_))))
                    {
                        self.dst_id = None;
                    }
                };
            }
            Message::DstSelect(new_dst_id) => {
                self.dst_id = Some(new_dst_id);
                if let Some(src_id) = self.src_id {
                    if new_dst_id == src_id
                        || (matches!(self.devices.get(&new_dst_id), Some(Device::Network(_)))
                            && matches!(self.devices.get(&src_id), Some(Device::Network(_))))
                    {
                        self.src_id = None;
                    }
                };
            }
            Message::PartSelect(index) => {
                let _ = comm!(openpartition, proto::usbsas::RequestOpenPartition { index });
                return self.update(Message::ReadDir("/".into()));
            }
            Message::ReadDir(path) => {
                let rep = comm!(
                    readdir,
                    proto::usbsas::RequestReadDir { path: path.clone() }
                );
                self.current_dir = path;
                self.current_files = Vec::new();
                rep.filesinfo
                    .iter()
                    .for_each(|x| self.current_files.push(x.path.clone()));
                self.state = State::ReadDir(rep.filesinfo);
            }
            Message::PreviousDir => {
                if let Some(index) = self.current_dir.rfind('/') {
                    if index != 0 {
                        let (new_dir, _) = self.current_dir.split_at(index);
                        self.current_dir = new_dir.into();
                    } else {
                        self.current_dir = "/".into();
                    };
                };
                return self.update(Message::ReadDir(self.current_dir.clone()));
            }
            Message::SelectFile(path) => {
                self.selected.insert(path);
            }
            Message::UnSelectFile(path) => {
                self.selected.remove(&path);
            }
            Message::SelectAll(mut files) => {
                while let Some(file) = files.pop() {
                    let _ = self.selected.insert(file);
                }
            }
            Message::EmptySelect(files) => {
                files.iter().for_each(|file| {
                    let _ = self.selected.remove(file);
                });
            }
            Message::Status((_, status)) => {
                if let Status::Progress(status) = status {
                    self.seen_status.insert(
                        status
                            .status
                            .try_into()
                            .unwrap_or(proto::common::Status::Unknown),
                    );
                };
                self.state = State::Status(status.clone());
                if let Status::Progress(status) = &status {
                    if let Ok(proto::common::Status::AllDone) = &status.status.try_into() {
                        self.report = comm!(report, proto::usbsas::RequestReport {}).report;
                        if let Some(ref report) = self.report {
                            log::debug!("{:#?}", &report);
                            if let Some(ref report_conf) = self.config.report {
                                if let Some(path) = &report_conf.write_local {
                                    let filename = format!(
                                        "usbsas_report_{}_{}.json",
                                        report.datetime, report.transfer_id
                                    );
                                    if let Ok(mut file) =
                                        fs::File::create(path::Path::new(path).join(filename))
                                    {
                                        if let Err(err) =
                                            serde_json::to_writer_pretty(&mut file, report)
                                        {
                                            log::error!("Error writing report: {}", err);
                                        };
                                    };
                                };
                            };
                        };
                        self.state = State::Done;
                    }
                }
            }
            Message::Tick(_) => match self.state {
                State::Init | State::DevSelect | State::Wipe(_) | State::DiskImg | State::Done => {
                    self.try_connect();
                    if self.comm.is_some() {
                        return self.update(Message::Devices);
                    }
                }
                State::UserID => {
                    if self.comm.is_some() {
                        return self.update(Message::UserID);
                    }
                }
                _ => (),
            },
            Message::Reset => return self.reset(),
        }

        if self.fullscreen {
            window::get_latest().and_then(move |window| {
                window::get_mode(window).then(move |mode| {
                    if mode != Mode::Fullscreen {
                        window::change_mode(window, Mode::Fullscreen)
                    } else {
                        iced::Task::none()
                    }
                })
            })
        } else {
            iced::Task::none()
        }
    }
}
