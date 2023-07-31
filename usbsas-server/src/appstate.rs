use crate::error::{AuthentError, ServiceError};
use crate::outfiles::OutFiles;
use actix_web::web;
use base64::{engine as b64eng, Engine as _};
use futures::task::{Context, Poll, Waker};
use hmac::{Hmac, Mac};
use log::{debug, error};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
};
use usbsas_comm::{protorequest, Comm};
use usbsas_config::{conf_parse, conf_read, Config};
use usbsas_process::UsbsasChildSpawner;
use usbsas_proto as proto;
use usbsas_proto::common::{OutFileType, OutFsType};

protorequest!(
    CommUsbsas,
    usbsas,
    id = Id[RequestId, ResponseId],
    postcopycmd = PostCopyCmd[RequestPostCopyCmd, ResponsePostCopyCmd],
    devices = Devices[RequestDevices, ResponseDevices],
    opendev = OpenDevice[RequestOpenDevice, ResponseOpenDevice],
    partitions = Partitions[RequestPartitions, ResponsePartitions],
    openpartition = OpenPartition[RequestOpenPartition, ResponseOpenPartition],
    readdir = ReadDir[RequestReadDir, ResponseReadDir],
    getattr = GetAttr[RequestGetAttr, ResponseGetAttr],
    wipe = Wipe[RequestWipe, ResponseWipe],
    imgdisk = ImgDisk[RequestImgDisk, ResponseImgDisk],
    report = Report[RequestReport, ResponseReport],
    end = End[RequestEnd, ResponseEnd]
);

/// Private device structures, they contain elements which should not be leaked
/// to the web clients (busnum, devnum etc.)
type UsbDevice = proto::common::Device;
type NetDevice = usbsas_config::Network;
type CmdDevice = usbsas_config::Command;

#[derive(Debug)]
enum Device {
    Usb(UsbDevice),
    Net(NetDevice),
    Cmd(CmdDevice),
}

#[derive(Debug)]
pub(crate) struct TargetDevice {
    device: Device,
    is_src: bool,
    is_dst: bool,
}

enum Destination {
    Usb {
        busnum: u32,
        devnum: u32,
    },
    Net {
        url: String,
        krb_service_name: Option<String>,
    },
    Cmd,
}

enum Source {
    Usb {
        opendev: proto::usbsas::RequestOpenDevice,
    },
    Net,
}

/// Public device structures we can send to web clients.

#[derive(Serialize, Debug)]
pub(crate) struct UsbsasInfos {
    pub(crate) name: String,
    pub(crate) message: String,
    pub(crate) version: String,
    pub(crate) status: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum DevType {
    Usb,
    Net,
    Cmd,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Partition {
    pub index: usize,
    size: u64,
    start: u64,
    ptype: u32,
    pub type_str: String,
    name_str: String,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct ReadDir {
    pub ftype: i32,
    size: u64,
    timestamp: i64,
    pub path: String,
    pub path_display: String,
    path_parent: String,
    path_parent_display: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct USBDesc {
    vendorid: u32,
    productid: u32,
    manufacturer: String,
    serial: String,
    description: String,
    is_src: bool,
    is_dst: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NetDesc {
    longdescr: String,
    description: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CmdDesc {
    longdescr: String,
    description: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Desc {
    Usb(USBDesc),
    Net(NetDesc),
    Cmd(CmdDesc),
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeviceDesc {
    pub dev: Desc,
    pub id: String,
    pub is_src: bool,
    pub is_dst: bool,
    pub dev_type: DevType,
}

impl From<&TargetDevice> for DeviceDesc {
    fn from(target: &TargetDevice) -> DeviceDesc {
        match target.device {
            Device::Net(ref net) => {
                let net_json = NetDesc {
                    longdescr: net.longdescr.to_owned(),
                    description: net.description.to_owned(),
                };
                let desc_json = Desc::Net(net_json);
                DeviceDesc {
                    dev: desc_json,
                    id: net.fingerprint(),
                    is_src: target.is_src,
                    is_dst: target.is_dst,
                    dev_type: DevType::Net,
                }
            }
            Device::Cmd(ref cmd) => {
                let cmd_json = CmdDesc {
                    longdescr: cmd.longdescr.to_owned(),
                    description: cmd.description.to_owned(),
                };
                let desc_json = Desc::Cmd(cmd_json);
                DeviceDesc {
                    dev: desc_json,
                    id: cmd.fingerprint(),
                    is_src: target.is_src,
                    is_dst: target.is_dst,
                    dev_type: DevType::Cmd,
                }
            }
            Device::Usb(ref usb) => {
                let net_json = USBDesc {
                    vendorid: usb.vendorid,
                    productid: usb.productid,
                    manufacturer: usb.manufacturer.to_owned(),
                    serial: usb.serial.to_owned(),
                    description: usb.description.to_owned(),
                    is_src: target.is_src,
                    is_dst: target.is_dst,
                };

                let desc_json = Desc::Usb(net_json);
                DeviceDesc {
                    dev: desc_json,
                    id: usb.fingerprint(),
                    is_src: target.is_src,
                    is_dst: target.is_dst,
                    dev_type: DevType::Usb,
                }
            }
        }
    }
}

trait Fingerprinter {
    fn fingerprint(&self) -> String;
}

impl Fingerprinter for UsbDevice {
    fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"Usb:");
        hasher.update(self.busnum.to_le_bytes());
        hasher.update(self.devnum.to_le_bytes());
        hasher.update(&self.manufacturer);
        hasher.update(&self.description);
        hasher.update(&self.serial);
        format!("{:x}", hasher.finalize())
    }
}

impl Fingerprinter for NetDevice {
    fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"Net:");
        hasher.update(&self.description);
        hasher.update(&self.longdescr);
        hasher.update(&self.url);
        format!("{:x}", hasher.finalize())
    }
}

impl Fingerprinter for CmdDevice {
    fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"Cmd:");
        hasher.update(&self.description);
        hasher.update(&self.longdescr);
        hasher.update(&self.command_bin);
        for arg in &self.command_args {
            hasher.update(arg);
        }
        format!("{:x}", hasher.finalize())
    }
}

impl Device {
    fn fingerprint(&self) -> String {
        match self {
            Device::Usb(usb) => usb.fingerprint(),
            Device::Net(net) => net.fingerprint(),
            Device::Cmd(cmd) => cmd.fingerprint(),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct ReadDirQuery {
    pub(crate) path: String,
}

#[derive(Deserialize, Debug)]
pub(crate) struct CopyIn {
    pub(crate) selected: Vec<String>,
    pub(crate) fsfmt: String,
    pub(crate) download_pin: Option<String>,
}

#[derive(Serialize, Debug)]
struct ReportCopySize<'a> {
    status: &'a str,
    size: u64,
}

#[derive(Serialize, Debug)]
struct ReportDeviceSize<'a> {
    status: &'a str,
    current_size: u64,
    total_size: u64,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct ReportCopy<'a> {
    status: &'a str,
    pub error_path: Vec<String>,
    pub filtered_path: Vec<String>,
    pub dirty_path: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ReportProgress<'a> {
    status: &'a str,
    progress: f32,
}

#[derive(Serialize, Debug)]
struct ReportError<'a> {
    status: &'a str,
    msg: &'a str,
}

trait ReqAuthentication {
    fn verify(&self, hmac: &mut Hmac<Sha256>) -> Result<&[u8], AuthentError>;
    fn authent(&self, hmac: &mut Hmac<Sha256>) -> Vec<u8>;
}

impl ReqAuthentication for Vec<u8> {
    fn authent(&self, hmac: &mut Hmac<Sha256>) -> Vec<u8> {
        hmac.reset();
        hmac.update(self);
        let mut result = hmac.finalize_reset().into_bytes().to_vec();
        result.extend(self.iter());
        result
    }

    fn verify(&self, hmac: &mut Hmac<Sha256>) -> Result<&[u8], AuthentError> {
        hmac.reset();
        let length = Sha256::output_size();
        if self.len() < length {
            return Err(AuthentError::NotEnoughBytes);
        }
        let output = &self[length..];
        hmac.update(output);
        match hmac.clone().verify_slice(&self[..length]) {
            Ok(()) => Ok(output),
            Err(_) => Err(AuthentError::BadHmac),
        }
    }
}

/// Actix data struct
pub(crate) struct AppState {
    config: Mutex<Config>,
    pub config_path: Mutex<String>,
    comm: Mutex<Comm<proto::usbsas::Request>>,
    dest: Mutex<Option<Destination>>,
    hmac: Mutex<Hmac<Sha256>>,
    outfiles: Mutex<OutFiles>,
    pub status: Arc<RwLock<String>>,
    pub session_id: Arc<std::sync::RwLock<String>>,
}

impl AppState {
    pub(crate) fn new(config_path: String) -> Result<Self, ServiceError> {
        let config = conf_parse(&conf_read(&config_path)?)?;

        #[cfg(feature = "integration-tests")]
        let session_id = "00000000000000000000000000000000".to_string();
        #[cfg(not(feature = "integration-tests"))]
        let session_id = uuid::Uuid::new_v4().simple().to_string();

        let outfiles = OutFiles::new(config.out_directory.clone(), &session_id)?;

        // Create reports directory if it doesn't exists
        if let Some(report_config) = &config.report {
            if let Some(reports_dir) = &report_config.write_local {
                if let Err(err) = fs::create_dir(reports_dir) {
                    match err.kind() {
                        std::io::ErrorKind::AlreadyExists => (),
                        _ => return Err(err.into()),
                    }
                }
            };
        };

        debug!("Out tar file name: {:?}", outfiles.out_tar);
        debug!("Out fs file name: {:?}", outfiles.out_fs);

        let comm = AppState::start_usbsas(&config, &config_path, &outfiles, &session_id)?;

        Ok(AppState {
            config: Mutex::new(config),
            config_path: Mutex::new(config_path),
            outfiles: Mutex::new(outfiles),
            comm: Mutex::new(comm),
            dest: Mutex::new(None),
            hmac: Mutex::new(Hmac::new_from_slice(
                &rand::thread_rng().gen::<[u8; 0x10]>(),
            )?),
            status: Arc::new(RwLock::new(String::from("idle"))),
            session_id: Arc::new(RwLock::new(session_id)),
        })
    }

    fn start_usbsas(
        config: &Config,
        config_path: &str,
        outfiles: &OutFiles,
        session_id: &str,
    ) -> Result<Comm<proto::usbsas::Request>, ServiceError> {
        debug!("starting usbsas");

        let mut usbsas_cmd = UsbsasChildSpawner::new("usbsas-usbsas")
            .arg(&outfiles.out_tar)
            .arg(&outfiles.out_fs)
            .args(&["-c", config_path]);

        if config.analyzer.is_some() {
            usbsas_cmd = usbsas_cmd.arg("--analyze");
        }

        std::env::set_var("USBSAS_SESSION_ID", session_id);

        let usbsas_child = usbsas_cmd.spawn::<proto::usbsas::Request>()?;

        Ok(usbsas_child.comm)
    }

    pub(crate) fn reset(&self) -> Result<(), ServiceError> {
        let mut comm = self.comm.lock()?;
        let _ = comm.end(proto::usbsas::RequestEnd {})?;
        nix::sys::wait::wait()?;

        #[cfg(not(feature = "integration-tests"))]
        let new_session_id = uuid::Uuid::new_v4().simple().to_string();
        #[cfg(feature = "integration-tests")]
        let new_session_id = "0".to_string();

        self.outfiles.lock()?.reset(&new_session_id)?;

        let new_comm = AppState::start_usbsas(
            &*self.config.lock()?,
            &self.config_path.lock()?,
            &*self.outfiles.lock()?,
            &new_session_id,
        )?;

        *self.session_id.write()? = new_session_id;

        *comm = new_comm;

        Ok(())
    }

    pub fn list_usb_devices(&self) -> Result<Vec<TargetDevice>, ServiceError> {
        let mut comm = self.comm.lock()?;
        let mut devices = vec![];
        for device in comm.devices(proto::usbsas::RequestDevices {})?.devices {
            devices.push(TargetDevice {
                device: Device::Usb(device.clone()),
                is_src: device.is_src,
                is_dst: device.is_dst,
            });
        }
        Ok(devices)
    }

    fn list_alt_targets(&self) -> Result<Vec<TargetDevice>, ServiceError> {
        let config = self.config.lock()?;
        let mut targets = vec![];
        if let Some(networks) = &config.networks {
            for network in networks {
                targets.push(TargetDevice {
                    device: Device::Net(network.clone()),
                    is_src: false,
                    is_dst: true,
                });
            }
        }
        if let Some(cmd) = &config.command {
            targets.push(TargetDevice {
                device: Device::Cmd(cmd.clone()),
                is_src: false,
                is_dst: true,
            });
        }
        if let Some(source_network) = &config.source_network {
            targets.push(TargetDevice {
                device: Device::Net(source_network.clone()),
                is_src: true,
                is_dst: false,
            });
        }
        Ok(targets)
    }

    pub(crate) fn list_all_devices(&self) -> Result<Vec<TargetDevice>, ServiceError> {
        let mut target_devices = self.list_usb_devices()?;
        target_devices.append(&mut self.list_alt_targets()?);
        Ok(target_devices)
    }

    pub(crate) fn dev_from_fingerprint(
        &self,
        fingerprint: String,
    ) -> Result<UsbDevice, ServiceError> {
        for dev in self.list_usb_devices()? {
            if fingerprint == dev.device.fingerprint() {
                if let Device::Usb(usb) = dev.device {
                    return Ok(usb);
                }
            }
        }
        Err(ServiceError::Error("Couldn't find device".into()))
    }

    pub(crate) fn id(&self) -> Result<String, ServiceError> {
        Ok(self.comm.lock()?.id(proto::usbsas::RequestId {})?.id)
    }

    pub(crate) fn device_select(
        &self,
        fingerprint_in: String,
        fingerprint_out: String,
    ) -> Result<(), ServiceError> {
        if fingerprint_in == fingerprint_out {
            return Err(ServiceError::Error(
                "Output cannot be the same as input".to_string(),
            ));
        }

        let devices = self.list_all_devices()?;

        let mut in_dev = None;
        let mut dest = None;
        for dev in devices {
            let fingerprint = dev.device.fingerprint();
            if fingerprint_in == fingerprint {
                debug!("in_dev set");
                match &dev.device {
                    Device::Usb(ref usb) => {
                        in_dev = Some(Source::Usb {
                            opendev: proto::usbsas::RequestOpenDevice {
                                device: Some(usb.to_owned()),
                            },
                        });
                    }
                    Device::Net(_) => {
                        in_dev = Some(Source::Net);
                    }
                    Device::Cmd(_) => in_dev = None,
                }
            }
            if fingerprint_out == fingerprint {
                debug!("dest set");
                match &dev.device {
                    Device::Usb(ref usb) => {
                        dest = Some(Destination::Usb {
                            busnum: usb.busnum,
                            devnum: usb.devnum,
                        });
                    }
                    Device::Net(ref net) => {
                        dest = Some(Destination::Net {
                            url: net.url.clone(),
                            krb_service_name: net.krb_service_name.clone(),
                        })
                    }
                    Device::Cmd(_) => dest = Some(Destination::Cmd),
                }
            }
        }

        let in_dev = match (in_dev, &dest) {
            (Some(Source::Net), Some(_)) => {
                *self.dest.lock()? = dest;
                return Ok(());
            }
            (Some(Source::Usb { opendev }), Some(_)) => opendev,
            (_, _) => {
                error!("Cannot find in or out dev");
                return Err(ServiceError::Error("Cannot find in or out dev".to_string()));
            }
        };

        self.comm
            .lock()?
            .opendev(in_dev)
            .map_err(|err| ServiceError::Error(format!("couldn't open input device: {err}")))?;
        *self.dest.lock()? = dest;

        Ok(())
    }

    pub(crate) fn read_partitions(&self) -> Result<Vec<Partition>, ServiceError> {
        match self
            .comm
            .lock()?
            .partitions(proto::usbsas::RequestPartitions {})
        {
            Ok(partitions) => Ok(partitions
                .partitions
                .iter()
                .enumerate()
                .map(|(i, partition)| Partition {
                    index: i,
                    size: partition.size,
                    start: partition.start,
                    ptype: partition.ptype,
                    type_str: partition.type_str.to_string(),
                    name_str: partition.name_str.to_string(),
                })
                .collect()),
            Err(err) => {
                error!("Couldn't read partitions: {}", err);
                Err(ServiceError::InternalServerError)
            }
        }
    }

    pub(crate) fn open_partition(&self, index: u32) -> Result<(), ServiceError> {
        if let Err(err) = self
            .comm
            .lock()?
            .openpartition(proto::usbsas::RequestOpenPartition { index })
        {
            error!("Error opening partition: {}", err);
            return Err(ServiceError::Error(format!("Cannot open partition: {err}")));
        };
        Ok(())
    }

    pub(crate) fn read_dir(&self, path: &str) -> Result<Vec<ReadDir>, ServiceError> {
        let parent_path_b64 = path.replace(' ', "+");
        let mut parent_path = b64eng::general_purpose::STANDARD_NO_PAD.decode(&parent_path_b64)?;
        let mut hmac = self.hmac.lock()?;

        if !parent_path.is_empty() {
            parent_path = parent_path.verify(&mut hmac)?.to_vec();
        }

        let parent_path_str = String::from_utf8(parent_path)?;

        let dir_info = self.comm.lock()?.readdir(proto::usbsas::RequestReadDir {
            path: parent_path_str.clone(),
        })?;

        // Build information for each element in current path
        let mut files = Vec::new();
        for infos in dir_info.filesinfo {
            let path_b64 = b64eng::general_purpose::STANDARD_NO_PAD
                .encode(infos.path.clone().into_bytes().authent(&mut hmac))
                .replace('\n', "");
            files.push(ReadDir {
                ftype: infos.ftype,
                size: infos.size,
                timestamp: infos.timestamp,
                path: path_b64,
                path_display: infos.path,
                path_parent: parent_path_b64.clone(),
                path_parent_display: parent_path_str.clone(),
            })
        }
        Ok(files)
    }

    pub(crate) fn copy(
        &self,
        req_selected: Vec<String>,
        fsfmt: String,
        download_pin: Option<String>,
        resp_stream: ResponseStream,
    ) -> Result<(), ServiceError> {
        use proto::usbsas::response::Msg;
        let mut src_is_net = false;

        let source = match download_pin {
            Some(pin) => {
                let pin = pin
                    .parse::<u64>()
                    .map_err(|_| ServiceError::InternalServerError)?;
                src_is_net = true;
                Some(proto::usbsas::request_copy_start::Source::SrcNet(
                    proto::usbsas::SrcNet { pin },
                ))
            }
            None => Some(proto::usbsas::request_copy_start::Source::SrcUsb(
                proto::usbsas::SrcUsb {},
            )),
        };

        let mut progress = 0.0;
        let mut resp_stream = resp_stream;
        let mut hmac = self.hmac.lock()?;
        let mut selected: Vec<String> = Vec::new();
        for path in &req_selected {
            selected.push(String::from_utf8(
                b64eng::general_purpose::STANDARD_NO_PAD
                    .decode(path)?
                    .verify(&mut hmac)?
                    .to_vec(),
            )?);
        }
        selected.sort();
        drop(hmac);

        let mut comm = self.comm.lock()?;
        resp_stream.report_progress("copy_start", progress)?;

        let dest = self.dest.lock()?;
        let destination = match dest.as_ref().ok_or(ServiceError::InternalServerError)? {
            Destination::Usb { busnum, devnum } => {
                debug!("do copy usb {} {} ({})", busnum, devnum, fsfmt);
                let fstype = match fsfmt.as_str() {
                    "ntfs" => OutFsType::Ntfs,
                    "exfat" => OutFsType::Exfat,
                    "fat32" => OutFsType::Fat,
                    _ => return Err(ServiceError::InternalServerError),
                };
                proto::usbsas::request_copy_start::Destination::Usb(proto::usbsas::DestUsb {
                    busnum: *busnum,
                    devnum: *devnum,
                    fstype: fstype.into(),
                })
            }
            Destination::Net {
                url,
                krb_service_name,
            } => {
                debug!("do copy net");
                proto::usbsas::request_copy_start::Destination::Net(proto::common::DestNet {
                    url: url.to_owned(),
                    krb_service_name: krb_service_name.clone().unwrap_or_else(|| String::from("")),
                })
            }
            Destination::Cmd { .. } => {
                debug!("do copy cmd");
                proto::usbsas::request_copy_start::Destination::Cmd(proto::usbsas::DestCmd {})
            }
        };

        progress += 1.0;
        resp_stream.report_progress("copy_usb_read_attrs", progress)?;
        progress += 1.0;
        resp_stream.report_progress("copy_usb_filter", progress)?;

        let write_report = if let Some(report_conf) = &self.config.lock()?.report {
            report_conf.write_dest
        } else {
            false
        };

        comm.send(proto::usbsas::Request {
            msg: Some(proto::usbsas::request::Msg::CopyStart(
                proto::usbsas::RequestCopyStart {
                    destination: Some(destination),
                    selected,
                    source,
                    write_report,
                },
            )),
        })?;

        let mut size_read = 0;
        let mut total_size = 0;
        let mut current_progress = progress;
        let mut resp: proto::usbsas::Response = comm.recv()?;
        // tar src files
        loop {
            match resp.msg.ok_or(ServiceError::InternalServerError)? {
                Msg::CopyStart(msg) => {
                    total_size = msg.total_files_size;
                    progress += 1.0;
                    resp_stream.report_progress("copy_usb_tar_start", progress)?;
                }
                Msg::CopyStatus(msg) => {
                    size_read += msg.current_size;
                    progress = current_progress + (size_read as f32 / total_size as f32 * 30.0);
                    resp_stream.report_progress("copy_usb_tar_update", progress)?;
                }
                Msg::CopyStatusDone(_) => break,
                Msg::NotEnoughSpace(msg) => {
                    resp_stream.report_progress("copy_usb_tar_start", progress)?;
                    resp_stream.add_message(ReportCopySize {
                        status: "copy_not_enough_space",
                        size: msg.max_size,
                    })?;
                    resp_stream.done()?;
                    return Ok(());
                }
                Msg::NothingToCopy(msg) => {
                    resp_stream.add_message(ReportCopy {
                        status: "nothing_to_copy",
                        filtered_path: msg.rejected_filter,
                        dirty_path: msg.rejected_dirty,
                        error_path: vec![],
                    })?;
                    resp_stream.done()?;
                    return Ok(());
                }
                Msg::Error(err) => {
                    error!("{}", err.err);
                    resp_stream.report_error(&err.err)?;
                    return Err(ServiceError::InternalServerError);
                }
                _ => {
                    resp_stream.report_error("Unexpected response from usbsas")?;
                    return Err(ServiceError::InternalServerError);
                }
            }
            resp = comm.recv()?;
        }
        progress = current_progress + 30.0;

        if self.config.lock()?.analyzer.is_some() && !src_is_net {
            if let Some(Destination::Usb { .. }) = *dest {
                resp_stream.report_progress("analyzing", progress)?;
                current_progress = progress;
                loop {
                    resp = comm.recv()?;
                    match resp.msg.ok_or(ServiceError::InternalServerError)? {
                        Msg::AnalyzeStatus(msg) => {
                            progress = current_progress
                                + (msg.current_size as f32 / msg.total_size as f32 * 5.0);
                            resp_stream.report_progress("analyze_update", progress)?;
                        }
                        Msg::AnalyzeDone(_) => break,
                        Msg::Error(err) => {
                            resp_stream.report_error(&err.err)?;
                            return Err(ServiceError::InternalServerError);
                        }
                        _ => {
                            error!("Unexpected resp");
                            resp_stream.report_error("Unexpected response from usbsas")?;
                            return Err(ServiceError::InternalServerError);
                        }
                    }
                }
                progress = current_progress + 5.0;
            };
        };

        size_read = 0;
        current_progress = progress;

        match dest.as_ref().ok_or(ServiceError::InternalServerError)? {
            Destination::Usb { .. } => {
                resp_stream.report_progress("copy_fromtar_tofs", progress)?;
                // create fs
                loop {
                    resp = comm.recv()?;
                    match resp.msg.ok_or(ServiceError::InternalServerError)? {
                        Msg::CopyStatus(msg) => {
                            size_read += msg.current_size;
                            progress =
                                current_progress + (size_read as f32 / total_size as f32 * 30.0);
                            resp_stream.report_progress("copy_fromtar_update", progress)?;
                        }
                        Msg::CopyStatusDone(_) => break,
                        Msg::NothingToCopy(msg) => {
                            resp_stream.add_message(ReportCopy {
                                status: "nothing_to_copy",
                                filtered_path: msg.rejected_filter,
                                dirty_path: msg.rejected_dirty,
                                error_path: vec![],
                            })?;
                            resp_stream.done()?;
                            return Ok(());
                        }
                        Msg::Error(err) => {
                            error!("{}", err.err);
                            resp_stream.report_error(&err.err)?;
                            return Err(ServiceError::InternalServerError);
                        }
                        _ => {
                            resp_stream.report_error("Unexpected response from usbsas")?;
                            return Err(ServiceError::InternalServerError);
                        }
                    }
                }
                progress = current_progress + 30.0;
                resp_stream.report_progress("copy_fs2dev_start", progress)?
            }
            Destination::Net { .. } => {
                progress = current_progress + 30.0;
                resp_stream.report_progress("copy_upload_start", progress)?
            }
            Destination::Cmd { .. } => {
                progress = current_progress + 30.0;
                resp_stream.report_progress("copy_cmd_start", progress)?
            }
        }

        progress += 1.0;
        current_progress = progress;

        // fs2dev or upload
        let final_report = loop {
            resp = comm.recv()?;
            match resp.msg.ok_or(ServiceError::InternalServerError)? {
                Msg::FinalCopyStatus(msg) => {
                    if msg.total_size != 0 && msg.current_size != 0 {
                        progress = current_progress
                            + (msg.current_size as f32 / msg.total_size as f32 * 30.0);
                        resp_stream.report_progress("copy_final_update", progress)?;
                    }
                }
                Msg::FinalCopyStatusDone(_) => {
                    // wait for response copy to break
                    continue;
                }
                Msg::CopyDone(info) => {
                    progress = current_progress + 30.0;
                    resp_stream.report_progress("terminate", progress)?;
                    break ReportCopy {
                        status: "final_report",
                        error_path: info.error_path,
                        filtered_path: info.filtered_path,
                        dirty_path: info.dirty_path,
                    };
                }
                Msg::Error(err) => {
                    error!("{}", err.err);
                    resp_stream.report_error(&err.err)?;
                    return Err(ServiceError::InternalServerError);
                }
                _ => {
                    error!("Unexpected response from usbsas");
                    resp_stream.report_error("Unexpected reposne from usbsas")?;
                    return Err(ServiceError::InternalServerError);
                }
            }
        };

        if let Some(report_conf) = &self.config.lock()?.report {
            if let Some(report_dir) = &report_conf.write_local {
                // save report on local disk
                let transfer_report = comm.report(proto::usbsas::RequestReport {})?.report;
                let datetime = time::OffsetDateTime::now_utc();
                let report_file_name = format!(
                    "usbsas_transfer_{:04}{:02}{:02}{:02}{:02}{:02}_{}.json",
                    datetime.year(),
                    datetime.month() as u8,
                    datetime.day(),
                    datetime.hour(),
                    datetime.minute(),
                    datetime.second(),
                    self.session_id.read()?,
                );
                let mut report_file =
                    fs::File::create(path::Path::new(&report_dir).join(report_file_name))?;
                report_file.write_all(&transfer_report)?;
            }
        }

        // post copy cmd
        if let Some(usbsas_config::PostCopy { .. }) = self.config.lock()?.post_copy {
            let outfiletype = match dest.as_ref().ok_or(ServiceError::InternalServerError)? {
                Destination::Usb { .. } => OutFileType::Fs,
                Destination::Net { .. } | Destination::Cmd { .. } => OutFileType::Tar,
            };
            comm.postcopycmd(proto::usbsas::RequestPostCopyCmd {
                outfiletype: outfiletype.into(),
            })?;
        };

        resp_stream.add_message(final_report)?;
        resp_stream.done()?;
        Ok(())
    }

    pub(crate) fn wipe(
        &self,
        device: UsbDevice,
        fsfmt: String,
        quick: bool,
        resp_stream: ResponseStream,
    ) -> Result<(), ServiceError> {
        use proto::usbsas::response::Msg;

        let mut resp_stream = resp_stream;
        resp_stream.report_progress("wipe_start", 0.0)?;

        let fstype = match fsfmt.as_str() {
            "ntfs" => OutFsType::Ntfs,
            "exfat" => OutFsType::Exfat,
            "fat32" => OutFsType::Fat,
            _ => return Err(ServiceError::InternalServerError),
        };

        let mut comm = self.comm.lock()?;
        comm.send(proto::usbsas::Request {
            msg: Some(proto::usbsas::request::Msg::Wipe(
                proto::usbsas::RequestWipe {
                    busnum: device.busnum,
                    devnum: device.devnum,
                    fstype: fstype.into(),
                    quick,
                },
            )),
        })?;

        loop {
            let resp: proto::usbsas::Response = comm.recv()?;
            match resp.msg.ok_or(ServiceError::InternalServerError)? {
                Msg::FinalCopyStatus(ref msg) => resp_stream.add_message(ReportDeviceSize {
                    status: "wipe_status",
                    current_size: msg.current_size,
                    total_size: msg.total_size,
                })?,
                Msg::FinalCopyStatusDone(_) => resp_stream.add_message(ReportDeviceSize {
                    status: "format_status",
                    current_size: 0,
                    total_size: 0,
                })?,
                Msg::Error(err) => {
                    error!("Wipe error: {}", err.err);
                    resp_stream.report_error(&err.err)?;
                    return Err(ServiceError::InternalServerError);
                }
                Msg::Wipe(_) => {
                    resp_stream.report_progress("wipe_end", 0.0)?;
                    resp_stream.done()?;
                    break;
                }
                _ => {
                    error!("Unexpected response from usbsas");
                    resp_stream.report_error("Unexpected reposne from usbsas")?;
                    return Err(ServiceError::InternalServerError);
                }
            }
        }

        Ok(())
    }

    pub(crate) fn imagedisk(
        &self,
        device: UsbDevice,
        resp_stream: ResponseStream,
    ) -> Result<(), ServiceError> {
        use proto::usbsas::response::Msg;

        let mut resp_stream = resp_stream;
        resp_stream.report_progress("imgdisk_start", 0.0)?;

        let mut comm = self.comm.lock()?;

        comm.send(proto::usbsas::Request {
            msg: Some(proto::usbsas::request::Msg::ImgDisk(
                proto::usbsas::RequestImgDisk {
                    device: Some(device.to_owned()),
                },
            )),
        })?;

        loop {
            let resp: proto::usbsas::Response = comm.recv()?;
            match resp.msg.ok_or(ServiceError::InternalServerError)? {
                Msg::OpenDevice(_) => continue,
                Msg::FinalCopyStatus(msg) => resp_stream.add_message(ReportDeviceSize {
                    status: "imgdisk_update",
                    current_size: msg.current_size,
                    total_size: msg.total_size,
                })?,
                Msg::ImgDisk(_) => {
                    // Keep out fs
                    let datetime = time::OffsetDateTime::now_utc();
                    fs::rename(
                        self.outfiles.lock()?.out_fs.clone(),
                        format!(
                            "{}/imgdisk_{:04}{:02}{:02}{:02}{:02}{:02}_{}_{}_{}.bin",
                            self.config.lock()?.out_directory,
                            datetime.year(),
                            datetime.month() as u8,
                            datetime.day(),
                            datetime.hour(),
                            datetime.minute(),
                            datetime.second(),
                            device.serial,
                            device.vendorid,
                            device.productid
                        ),
                    )?;
                    resp_stream.report_progress("imgdisk_end", 0.0)?;
                    resp_stream.done()?;
                    break;
                }
                Msg::Error(err) => {
                    error!("{}", err.err);
                    resp_stream.report_error(&err.err)?;
                    return Err(ServiceError::InternalServerError);
                }
                _ => {
                    error!("Unexpected response from usbsas");
                    resp_stream.report_error("Unexpected reposne from usbsas")?;
                    return Err(ServiceError::InternalServerError);
                }
            }
        }

        Ok(())
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        // End usbsas and its children properly
        let mut comm = self.comm.lock().unwrap();
        let _ = comm.end(proto::usbsas::RequestEnd {}).ok();
        nix::sys::wait::wait().unwrap();
    }
}

/// Struct that impl futures::Stream to report progress to the client
#[derive(Clone)]
pub(crate) struct ResponseStream {
    /// Contains serialized messages to send
    messages: Arc<Mutex<Vec<u8>>>,
    done: Arc<AtomicBool>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl ResponseStream {
    pub(crate) fn new() -> Self {
        ResponseStream {
            messages: Arc::new(Mutex::new(Vec::new())),
            done: Arc::new(AtomicBool::new(false)),
            waker: Arc::new(Mutex::new(None)),
        }
    }

    fn add_serialized_message(&mut self, message: &mut Vec<u8>) -> Result<(), ServiceError> {
        let mut messages = self.messages.lock()?;
        messages.append(message);
        // Also append "\r\n" in case multiple json messages are added between 2 polls
        messages.append(&mut vec![13, 10]);
        drop(messages);
        if let Some(waker) = self.waker.lock()?.take() {
            waker.wake();
        }
        Ok(())
    }

    fn add_message<T: Serialize>(&mut self, message: T) -> Result<(), ServiceError> {
        self.add_serialized_message(&mut serde_json::to_vec(&message)?)
    }

    fn report_progress(&mut self, status: &str, progress: f32) -> Result<(), ServiceError> {
        self.add_message(ReportProgress { status, progress })
    }

    fn report_error(&mut self, msg: &str) -> Result<(), ServiceError> {
        self.add_message(ReportError {
            status: "fatal_error",
            msg,
        })?;
        self.done()
    }

    pub fn done(&mut self) -> Result<(), ServiceError> {
        self.done.store(true, Ordering::Relaxed);
        if let Some(waker) = self.waker.lock()?.take() {
            waker.wake();
        }
        Ok(())
    }
}

impl Drop for ResponseStream {
    fn drop(&mut self) {
        self.done().ok();
    }
}

impl futures::Stream for ResponseStream {
    type Item = Result<web::Bytes, actix_web::Error>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.messages.lock().unwrap().len() == 0 {
            if self.done.load(Ordering::Relaxed) {
                return Poll::Ready(None);
            } else {
                *self.waker.lock().unwrap() = Some(cx.waker().clone());
                return Poll::Pending;
            }
        }
        Poll::Ready(Some(Ok(web::Bytes::copy_from_slice(
            self.messages.lock().unwrap().drain(0..).as_slice(),
        ))))
    }
}
