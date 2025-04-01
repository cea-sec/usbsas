use iced::time::Instant;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    os::unix::net::UnixStream,
    path::Path,
    sync::{Arc, Mutex},
};
use usbsas_comm::{Comm, ProtoReqCommon};
use usbsas_config::{conf_parse, conf_read};
use usbsas_proto::{
    self as proto,
    common::{device::Device, FileInfo, FsType, TransferReport},
};
use usbsas_utils::clap::UsbsasClap;

mod components;
mod subscription;
mod update;
mod view;

type Devices = BTreeMap<u64, Device>;
pub type ComRqUsbsas = Comm<proto::usbsas::Request, UnixStream, UnixStream>;

fn devices_from_proto(mut devs_proto: Vec<proto::common::Device>) -> Devices {
    let mut devices = BTreeMap::new();
    while let Some(dev_proto) = devs_proto.pop() {
        if let Some(dev) = dev_proto.device {
            devices.insert(dev_proto.id, dev);
        }
    }
    devices
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LANG {
    EN,
    FR,
}
static LANGS: &[LANG] = &[LANG::EN, LANG::FR];
const LANG_FR_DATA: &str = include_str!("../resources/i18n/fr.json");
const LANG_EN_DATA: &str = include_str!("../resources/i18n/en.json");

impl std::fmt::Display for LANG {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LANG::EN => write!(f, "🇬🇧"),
            LANG::FR => write!(f, "🇫🇷"),
        }
    }
}

static FSTYPES: &[FsType] = &[FsType::Ntfs, FsType::Exfat, FsType::Fat];

// 1536 == tar with only a "/data" entry (512b) + 1024b zeroes (created by files2tar when it starts)
const USBSAS_EMPTY_TAR: u64 = 1536;

#[derive(Debug, Clone)]
pub enum Status {
    Progress(proto::common::ResponseStatus),
    Error(String),
}

impl Status {
    fn init() -> Self {
        Status::Progress(proto::common::ResponseStatus {
            done: false,
            current: 0,
            total: 0,
            status: proto::common::Status::ReadSrc.into(),
        })
    }
}

#[derive(Debug)]
enum State {
    Init,
    DevSelect,
    UserID,
    PartSelect(Vec<proto::common::PartitionInfo>),
    ReadDir(Vec<FileInfo>),
    Status(Status),
    Wipe(bool),
    DiskImg,
    Done,
    DownloadPin,
    Tools,
    Faq,
    Reload,
    Error(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    Init,
    Faq,
    Tools,
    Ok,
    Nok,
    Devices,
    UserID,
    SrcSelect(u64),
    DstSelect(u64),
    PartSelect(u32),
    ReadDir(String),
    PreviousDir,
    SelectFile(String),
    UnSelectFile(String),
    SelectAll(Vec<String>),
    EmptySelect(Vec<String>),
    Status((usize, Status)),
    Wipe(bool),
    DiskImg,
    LangSelect(LANG),
    FsTypeSelect(FsType),
    Tick(Instant),
    UserInput(u8),
    ClearInput(bool),
    Reset,
}

impl Drop for GUI {
    fn drop(&mut self) {
        if let Some(comm) = &self.comm {
            match comm.lock() {
                Ok(mut guard) => {
                    if guard.end().is_err() {
                        log::error!("couldn't end usbsas");
                    }
                }
                Err(_) => log::error!("couldn't end usbsas"),
            }
        };
    }
}

pub fn client_clap() -> clap::Command {
    usbsas_utils::clap::new_usbsas_cmd("usbsas-client")
        .add_config_arg()
        .arg(
            clap::Arg::new("fullscreen")
                .short('f')
                .long("fullscreen")
                .help("Window in fullscreen mode")
                .num_args(0)
                .action(clap::ArgAction::SetTrue)
                .required(false),
        )
        .arg(
            clap::Arg::new("width")
                .value_name("WIDTH")
                .short('W')
                .long("width")
                .help("window width")
                .num_args(1)
                .default_value("1000")
                .value_parser(clap::value_parser!(u32)),
        )
        .arg(
            clap::Arg::new("height")
                .value_name("HEIGHT")
                .short('H')
                .long("height")
                .help("window height")
                .num_args(1)
                .default_value("800")
                .value_parser(clap::value_parser!(u32)),
        )
        .arg(
            clap::Arg::new("socket")
                .short('s')
                .long("socket")
                .value_name("SOCKET_PATH")
                .help("Unix domain socket path used to communicate")
                .num_args(1)
                .default_value(usbsas_utils::SOCKET_PATH)
                .required(false),
        )
}

pub struct GUI {
    comm: Option<Arc<Mutex<ComRqUsbsas>>>,
    state: State,
    devices: Devices,
    src_id: Option<u64>,
    dst_id: Option<u64>,
    userid: Option<String>,
    download_pin: Option<String>,
    selected: HashSet<String>,
    current_dir: String,
    current_files: Vec<String>,
    seen_status: BTreeSet<proto::common::Status>,
    version: String,
    config: usbsas_config::Config,
    report: Option<TransferReport>,
    status_title: Option<String>,
    i18n: HashMap<LANG, HashMap<String, String>>,
    lang: LANG,
    fstype: FsType,
    session_id: String,
    hostname: String,
    fullscreen: bool,
    socket_path: String,
}

use iced::Task;
//impl Default for GUI {
impl GUI {
    pub fn new() -> (Self, Task<Message>) {
        let matches = client_clap().get_matches();
        let config_path = matches.get_one::<String>("config").expect("config cmdline");
        let config = conf_parse(&conf_read(config_path).expect("can't read config"))
            .expect("can't parse config");

        let session_id = uuid::Uuid::new_v4().simple().to_string();
        std::env::set_var("USBSAS_SESSION_ID", &session_id);

        let lang = match config.lang {
            Some(ref opt) => match opt.as_str() {
                "en" => LANG::EN,
                "fr" => LANG::FR,
                _ => panic!("Unsupported lang: {opt}"),
            },
            None => LANG::EN,
        };

        if let Some(ref report_conf) = config.report {
            if let Some(path) = &report_conf.write_local {
                match fs::exists(path) {
                    Ok(true) => (),
                    Ok(false) => {
                        if let Err(err) = fs::create_dir(path) {
                            panic!("Can't create report directory \"{}\" ({})", path, err);
                        }
                    }
                    Err(err) => {
                        panic!("Can't check existence of \"{}\": {}", path, err);
                    }
                }
            };
        };

        let mut i18n = HashMap::new();
        let en: HashMap<String, String> =
            serde_json::from_str(LANG_EN_DATA).expect("can't parse lang json");
        i18n.insert(LANG::EN, en);
        let fr: HashMap<String, String> =
            serde_json::from_str(LANG_FR_DATA).expect("can't parse lang json");
        i18n.insert(LANG::FR, fr);

        let socket_path = matches
            .get_one::<String>("socket")
            .expect("clap socket")
            .to_string();

        (
            Self {
                comm: None,
                state: State::Init,
                devices: BTreeMap::new(),
                src_id: None,
                dst_id: None,
                userid: None,
                download_pin: None,
                current_dir: "".into(),
                current_files: Vec::new(),
                selected: HashSet::new(),
                seen_status: BTreeSet::new(),
                version: env!("CARGO_PKG_VERSION").into(),
                report: None,
                status_title: None,
                config,
                i18n,
                lang,
                fstype: FsType::Ntfs,
                session_id,
                hostname: sysinfo::System::host_name().unwrap_or("unknown".into()),
                fullscreen: *matches.get_one::<bool>("fullscreen").unwrap_or(&false),
                socket_path,
            },
            Task::done(Message::Init),
        )
    }
}

impl GUI {
    pub fn try_connect(&mut self) {
        if self.comm.is_none() {
            match UnixStream::connect(&self.socket_path) {
                Ok(stream) => {
                    self.comm = Some(Arc::new(Mutex::new(Comm::new(
                        stream.try_clone().unwrap(),
                        stream,
                    ))))
                }
                Err(err) => {
                    log::error!("couldn't connect: {err} {:?}", self.state);
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            };
        }
    }

    fn reset(&mut self) -> Task<Message> {
        if let Some(comm) = self.comm.take() {
            match comm.lock() {
                Ok(mut guard) => {
                    if guard.end().is_err() {
                        log::error!("couldn't end usbsas properly");
                    }
                }
                Err(_) => log::error!("couldn't end usbsas properly"),
            }
        };

        // Delete out & temp files if empty
        let tar_path = format!(
            "{}/usbsas_{}.tar",
            self.config.out_directory.trim_end_matches('/'),
            self.session_id,
        );
        let clean_tar_path = format!(
            "{}/usbsas_{}_clean.tar",
            self.config.out_directory.trim_end_matches('/'),
            self.session_id
        );
        let fs_path = format!(
            "{}/usbsas_{}.img",
            self.config.out_directory.trim_end_matches('/'),
            self.session_id,
        );
        if let Ok(metadata) = fs::metadata(&fs_path) {
            if metadata.len() == 0 {
                let _ = fs::remove_file(Path::new(&fs_path)).ok();
            }
        };

        for path in &[&tar_path, &clean_tar_path] {
            if let Ok(metadata) = fs::metadata(path) {
                if metadata.len() == USBSAS_EMPTY_TAR {
                    let _ = fs::remove_file(Path::new(&path)).ok();
                }
            };
        }

        let lang = self.lang.clone();
        let (new, task) = Self::new();
        *self = new;
        self.lang = lang;
        task
    }

    fn soft_reset(&mut self) {
        self.src_id = None;
        self.dst_id = None;
        self.download_pin = None;
    }
}
