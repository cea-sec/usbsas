use iced::{time::Instant, Task};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    os::unix::net::UnixStream,
    path::Path,
    sync::Arc,
};
use tokio::sync::Mutex;
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
    Sandbox,
    Connect,
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
    SysInfo,
    Reload,
    Error(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    Faq,
    Tools,
    Ok,
    Nok,
    Devices,
    UserID,
    SysInfo,
    SrcSelect(u64),
    DstSelect(u64),
    PartSelect(u32),
    ReadDir(String),
    PreviousDir,
    SelectFile(String),
    UnSelectFile(String),
    SelectAll(Vec<String>),
    UnselectAll(Vec<String>),
    Status(Status),
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
            if comm.blocking_lock().end().is_err() {
                log::error!("couldn't end usbsas properly");
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
        .arg(
            clap::Arg::new("nosandbox")
                .short('n')
                .long("nosandbox")
                .value_name("NO_SANDBOX")
                .help("Disable sandboxing the client")
                .num_args(0)
                .action(clap::ArgAction::SetTrue)
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
    fullscreen: bool,
    socket_path: String,
}

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
                            panic!("Can't create report directory \"{path}\" ({err})");
                        }
                    }
                    Err(err) => {
                        panic!("Can't check existence of \"{path}\": {err}");
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
                state: if *matches.get_one::<bool>("nosandbox").unwrap_or(&false) {
                    State::Connect
                } else {
                    State::Sandbox
                },
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
                fullscreen: *matches.get_one::<bool>("fullscreen").unwrap_or(&false),
                socket_path,
            },
            Task::none(),
        )
    }
}

impl GUI {
    fn sandbox(&mut self) {
        let mut paths_rw = vec!["/dev/dri", &self.socket_path];
        if let Some(report_conf) = &self.config.report {
            if let Some(path) = &report_conf.write_local {
                paths_rw.push(path);
            }
        };
        usbsas_sandbox::client::sandbox(
            Some(&[
                "/proc/cpuinfo",
                "/proc/uptime",
                "/proc/stat",
                "/proc/meminfo",
                "/proc/loadavg",
                "/proc/mounts",
                "/proc/diskstats",
                "/sys/class/net",
                "/etc/localtime",
            ]),
            Some(&paths_rw),
            None,
            None,
        )
        .expect("Unable to sandbox client");
        self.state = State::Connect;
    }

    fn try_connect(&mut self) {
        if self.comm.is_none() {
            match UnixStream::connect(&self.socket_path) {
                Ok(stream) => {
                    self.comm = Some(Arc::new(Mutex::new(Comm::new(
                        stream.try_clone().unwrap(),
                        stream,
                    ))));
                    self.state = State::Init;
                }
                Err(err) => {
                    log::error!("couldn't connect: {err} {:?}", self.state);
                }
            };
        } else {
            self.state = State::Init;
        }
    }

    fn reset(&mut self) -> Task<Message> {
        if let Some(comm) = self.comm.take() {
            let mut guard = comm.blocking_lock();
            if let Err(err) = guard.end() {
                log::error!("couldn't end usbsas properly: {}", err);
            }
            if let Err(err) = guard.input().shutdown(std::net::Shutdown::Both) {
                log::error!("couldn't shutdown socket: {}", err);
            };
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
                if let Err(err) = fs::remove_file(Path::new(&fs_path)) {
                    log::error!("couldn't rm file {}: {err}", &fs_path);
                };
            }
        };

        for path in &[&tar_path, &clean_tar_path] {
            if let Ok(metadata) = fs::metadata(path) {
                // Empty tar
                if metadata.len() == 1536 || metadata.len() == 512 {
                    if let Err(err) = fs::remove_file(Path::new(&path)) {
                        log::error!("couldn't rm file {}: {err}", path);
                    };
                }
            };
        }

        self.state = State::Connect;
        self.devices = BTreeMap::new();
        self.src_id = None;
        self.dst_id = None;
        self.userid = None;
        self.download_pin = None;
        self.current_dir = "".into();
        self.current_files = Vec::new();
        self.selected = HashSet::new();
        self.seen_status = BTreeSet::new();
        self.report = None;
        self.status_title = None;
        self.fstype = FsType::Ntfs;
        self.session_id = uuid::Uuid::new_v4().simple().to_string();
        std::env::set_var("USBSAS_SESSION_ID", &self.session_id);

        Task::none()
    }

    fn soft_reset(&mut self) {
        self.src_id = None;
        self.dst_id = None;
        self.download_pin = None;
    }
}
