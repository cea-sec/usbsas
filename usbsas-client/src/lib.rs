//! usbsas graphical user interface.
//!
//! The client is an [iced] application that drives a `usbsas` server through
//! its Unix domain socket. It doesn't access devices itself: every action
//! (listing devices, reading files, transfers, wipes...) is a protobuf request
//! sent to the server.
//!
//! It follows iced's architecture:
//! - [`GUI`] holds the application state, its current screen being a `State`,
//! - [`GUI::update`] handles [`Message`]s and returns [`Task`]s,
//! - [`GUI::view`] renders the current `State`,
//! - [`GUI::subscription`] emits a periodic [`Message::Tick`].
//!
//! Requests to the server never block the interface: they are run as async
//! tasks and their results come back as [`Message::Response`]. Each session
//! has a generation number, responses tagged with an older generation (sent
//! before a reset) are dropped.

use iced::{Task, time::Instant};
use smol::{Async, lock::Mutex};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    os::unix::net::UnixStream,
    path::Path,
    sync::Arc,
};
use usbsas_comm::{ComRqAsyncUsbsas, CommAsync};
use usbsas_config::{conf_parse, conf_read};
use usbsas_proto::{
    self as proto,
    common::{FileInfo, FsType, TransferReport, device::Device},
};
use usbsas_utils::clap::UsbsasClap;

mod components;
mod subscription;
mod update;
mod view;

/// Devices reported by usbsas, indexed by their id.
type Devices = BTreeMap<u64, Device>;
/// Async requester used to talk to the usbsas server over its Unix socket.
pub type ComRqUsbsas = ComRqAsyncUsbsas<Async<UnixStream>, Async<UnixStream>>;

/// Convert the device list of a `ResponseDevices` into [`Devices`], entries
/// without device are skipped.
fn devices_from_proto(mut devs_proto: Vec<proto::common::Device>) -> Devices {
    let mut devices = BTreeMap::new();
    while let Some(dev_proto) = devs_proto.pop() {
        if let Some(dev) = dev_proto.device {
            devices.insert(dev_proto.id, dev);
        }
    }
    devices
}

/// Languages of the interface. Translations are embedded from
/// `resources/i18n/*.json`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LANG {
    EN,
    FR,
}
/// Languages proposed in the language selector.
static LANGS: &[LANG] = &[LANG::EN, LANG::FR];
const LANG_FR_DATA: &str = include_str!("../resources/i18n/fr.json");
const LANG_EN_DATA: &str = include_str!("../resources/i18n/en.json");

/// Displayed as a flag in the language selector.
impl std::fmt::Display for LANG {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LANG::EN => write!(f, "🇬🇧"),
            LANG::FR => write!(f, "🇫🇷"),
        }
    }
}

/// File systems proposed for the destination device (transfer or wipe).
static FSTYPES: &[FsType] = &[FsType::Ntfs, FsType::Exfat, FsType::Fat];

/// Progress of a long running operation (transfer, wipe, disk image).
#[derive(Debug, Clone)]
pub enum Status {
    /// Status update received from usbsas
    Progress(proto::common::ResponseStatus),
    /// Error while receiving status, ends the operation
    Error(String),
}

impl Status {
    /// Initial status, displayed until the first update is received.
    fn init() -> Self {
        Status::Progress(proto::common::ResponseStatus {
            done: false,
            current: 0,
            total: 0,
            status: proto::common::Status::ReadSrc.into(),
        })
    }
}

/// Current step of the GUI, each one is rendered as a different screen.
///
/// The nominal flow of a transfer is:
/// `Sandbox` → `Connect` → `Init` → `DevSelect` (→ `UserID`) (→ `DownloadPin`)
/// → `PartSelect` → `ReadDir` → `Status` → `Done` → `Reload` → `Connect`...
#[derive(Debug)]
enum State {
    /// The client must sandbox itself (on next tick)
    Sandbox,
    /// Trying to connect to the usbsas socket (on each tick)
    Connect,
    /// Connected, waiting for a USB device to be plugged
    Init,
    /// Source and destination selection
    DevSelect,
    /// Waiting for the user to identify (usbsas couldn't get a user id yet)
    UserID,
    /// The source device has several partitions, the user must choose one
    PartSelect(Vec<proto::common::PartitionInfo>),
    /// Browsing the source, files of the current directory
    ReadDir(Vec<FileInfo>),
    /// Waiting for usbsas, the string is the i18n key of the text to display
    Wait(String),
    /// Long running operation in progress
    Status(Status),
    /// Wipe confirmation, `true` for a quick wipe
    Wipe(bool),
    /// Disk image confirmation
    DiskImg,
    /// Operation finished, report is displayed
    Done,
    /// Network source selected, the user must enter the download PIN
    DownloadPin,
    /// Tools menu (wipe, disk image, system info)
    Tools,
    /// Frequently asked questions
    Faq,
    /// System information
    SysInfo,
    /// Session is ending (see [`GUI::reset`])
    Reload,
    /// Unrecoverable error, the session must be reset
    Error(String),
}

/// Messages handled by [`GUI::update`], emitted by user interactions, the
/// subscription or completed tasks.
#[derive(Debug, Clone)]
pub enum Message {
    /// Show the FAQ
    Faq,
    /// Show the tools menu
    Tools,
    /// "Ok" / "Next" button, its effect depends on the current state
    Ok,
    /// "Cancel" / "Back" button, its effect depends on the current state
    Nok,
    /// Request the device list
    Devices,
    /// Request the user id
    UserID,
    /// Show system information
    SysInfo,
    /// Select the source device
    SrcSelect(u64),
    /// Select the destination device
    DstSelect(u64),
    /// Open the partition at this index of the source device
    PartSelect(u32),
    /// Start a transfer between the selected source and destination
    InitTransfer,
    /// List the content of a directory of the source
    ReadDir(String),
    /// Go to the parent of the current directory
    PreviousDir,
    /// Add a path to the selection
    SelectFile(String),
    /// Remove a path (and its children) from the selection
    UnSelectFile(String),
    /// Select all files of the current directory
    SelectAll(Vec<String>),
    /// Unselect all files of the current directory
    UnselectAll(Vec<String>),
    /// Send the selected files to usbsas, starting the copy
    ListFiles,
    /// Status update of the session with this generation
    Status(u64, Status),
    /// Show wipe confirmation, `true` for a quick wipe
    Wipe(bool),
    /// Show disk image confirmation
    DiskImg,
    /// Change the interface language
    LangSelect(LANG),
    /// Change the file system of the destination
    FsTypeSelect(FsType),
    /// Periodic tick from [`GUI::subscription`]
    Tick(Instant),
    /// Digit typed on the PIN pad
    UserInput(u8),
    /// Clear the PIN, entirely if `true`, last digit otherwise
    ClearInput(bool),
    /// End the current session
    Reset,
    /// The session with this generation has ended
    ResetDone(u64),
    /// Response of a request sent during the session with this generation
    Response(u64, Response),
}

/// Kind of source opened by a successful `InitTransfer` request, it decides
/// the next step.
#[derive(Debug, Clone)]
pub enum SrcOpened {
    /// USB device, with its partitions
    Usb(Vec<proto::common::PartitionInfo>),
    /// Network source, files are downloaded directly
    Network,
    /// Local directory, browsed like a USB partition
    LocalDir,
    Other,
}

/// Result of an async request sent to usbsas, delivered with
/// [`Message::Response`].
#[derive(Debug, Clone)]
pub enum Response {
    Devices(Result<Vec<proto::common::Device>, String>),
    UserId(Result<String, String>),
    InitTransfer(Result<SrcOpened, String>),
    OpenPartition(Result<(), String>),
    /// Content of the requested directory
    ReadDir(String, Result<Vec<FileInfo>, String>),
    /// A long running operation has started, status updates must be read
    StartStatus(Result<(), String>),
    /// Transfer report, received once the operation is done
    Report(Result<Option<TransferReport>, String>),
}

/// Try to end the usbsas session when the client exits.
impl Drop for GUI {
    fn drop(&mut self) {
        if let Some(comm) = self.comm.take() {
            match comm.try_lock() {
                Some(mut guard) => {
                    if smol::block_on(guard.end()).is_err() {
                        log::error!("couldn't end usbsas properly");
                    }
                }
                None => log::error!("request in flight, couldn't end usbsas properly"),
            }
        };
    }
}

/// Command line arguments of the client.
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
                .help("Usbsas socket path")
                .default_value(format!("{}/usbsas.sock", usbsas_utils::DEFAULT_SOCKET_DIR))
                .num_args(1)
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

/// The iced application.
pub struct GUI {
    /// Connection to usbsas, `None` until connected and during resets. Shared
    /// with the tasks sending requests.
    comm: Option<Arc<Mutex<ComRqUsbsas>>>,
    /// Incremented on each reset to discard messages of previous sessions
    generation: u64,
    /// A user-triggered request is in flight
    busy: bool,
    /// A devices request is in flight
    polling: bool,
    state: State,
    devices: Devices,
    src_id: Option<u64>,
    dst_id: Option<u64>,
    userid: Option<String>,
    /// PIN entered by the user to download files from a network source
    download_pin: Option<String>,
    /// Paths selected for the transfer (directories included)
    selected: HashSet<String>,
    current_dir: String,
    /// Paths of the files in `current_dir`
    current_files: Vec<String>,
    /// Steps already reached by the current operation, displayed as progress
    seen_status: BTreeSet<proto::common::Status>,
    version: String,
    config: usbsas_config::Config,
    report: Option<TransferReport>,
    /// i18n key of the title of the status screen (wipe, disk image...)
    status_title: Option<String>,
    i18n: HashMap<LANG, HashMap<String, String>>,
    lang: LANG,
    /// File system of the destination
    fstype: FsType,
    /// Unique id of the session, also exported as `USBSAS_SESSION_ID` and
    /// used in the names of the output files
    session_id: String,
    fullscreen: bool,
    socket_path: String,
}

impl GUI {
    /// Build the initial state from the command line and the configuration.
    ///
    /// # Panics
    ///
    /// Panics if the configuration can't be read or parsed, if its `lang` is
    /// unsupported or if the report directory can't be created.
    pub fn new() -> (Self, Task<Message>) {
        let matches = client_clap().get_matches();
        let config_path = matches.get_one::<String>("config").expect("config cmdline");
        let config = conf_parse(&conf_read(config_path).expect("can't read config"))
            .expect("can't parse config");

        let session_id = uuid::Uuid::new_v4().simple().to_string();
        unsafe { std::env::set_var("USBSAS_SESSION_ID", &session_id) };

        let lang = match config.lang {
            Some(ref opt) => match opt.as_str() {
                "en" => LANG::EN,
                "fr" => LANG::FR,
                _ => panic!("Unsupported lang: {opt}"),
            },
            None => LANG::EN,
        };

        if let Some(ref report_conf) = config.report
            && let Some(path) = &report_conf.write_local
        {
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

        let mut i18n = HashMap::new();
        let en: HashMap<String, String> =
            serde_json::from_str(LANG_EN_DATA).expect("can't parse lang json");
        i18n.insert(LANG::EN, en);
        let fr: HashMap<String, String> =
            serde_json::from_str(LANG_FR_DATA).expect("can't parse lang json");
        i18n.insert(LANG::FR, fr);

        let socket_path = matches.get_one::<String>("socket").unwrap().to_string();

        (
            Self {
                comm: None,
                generation: 0,
                busy: false,
                polling: false,
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
    /// Restrict the client with landlock and seccomp, only the paths it needs
    /// remain accessible. Called once, on the first tick, so that iced is
    /// initialized before.
    fn sandbox(&mut self) {
        let mut paths_rw = vec!["/dev/dri", &self.socket_path];
        if let Some(report_conf) = &self.config.report
            && let Some(path) = &report_conf.write_local
        {
            paths_rw.push(path);
        };
        let paths_rm: Option<&[&str]> = if let Some(false) = self.config.keep_tmp_files {
            Some(&[self.config.out_directory.as_str()])
        } else {
            None
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
            paths_rm,
        )
        .expect("Unable to sandbox client");
        self.state = State::Connect;
    }

    /// Try to connect to the usbsas socket, switching to `State::Init` on
    /// success. Called on each tick until the server is available.
    fn try_connect(&mut self) {
        if self.comm.is_none() {
            // connect() on a unix socket doesn't block, no need to make it async
            let connect = || -> std::io::Result<ComRqUsbsas> {
                let stream = UnixStream::connect(&self.socket_path)?;
                Ok(CommAsync::new(
                    Async::new(stream.try_clone()?)?,
                    Async::new(stream)?,
                ))
            };
            match connect() {
                Ok(comm) => {
                    self.comm = Some(Arc::new(Mutex::new(comm)));
                    self.state = State::Init;
                }
                Err(err) => {
                    if err.kind() == std::io::ErrorKind::NotFound {
                        log::debug!("Waiting usbsas socket...");
                    } else {
                        log::error!("couldn't connect: {err} {:?}", self.state);
                    }
                }
            };
        } else {
            self.state = State::Init;
        }
    }

    /// End the usbsas session asynchronously, the GUI state is reset when it's
    /// done (`Message::ResetDone`).
    fn reset(&mut self) -> Task<Message> {
        // From now on, responses of requests sent during the ending session
        // must be ignored
        self.generation = self.generation.wrapping_add(1);
        self.busy = false;
        self.polling = false;
        let generation = self.generation;

        match self.comm.take() {
            Some(comm) => Task::perform(
                async move {
                    // Waits for a request still in flight before sending End
                    let mut guard = comm.lock().await;
                    if let Err(err) = guard.end().await {
                        log::error!("couldn't end usbsas properly: {}", err);
                    }
                    if let Err(err) = guard.input().get_ref().shutdown(std::net::Shutdown::Both) {
                        log::error!("couldn't shutdown socket: {}", err);
                    };
                },
                move |_| Message::ResetDone(generation),
            ),
            None => Task::done(Message::ResetDone(generation)),
        }
    }

    /// Finish a reset started by [`GUI::reset`]: remove empty (or all, if
    /// `keep_tmp_files` is false) output files of the session, clear the
    /// session state and start a new session.
    fn reset_done(&mut self, generation: u64) -> Task<Message> {
        if generation != self.generation {
            // A newer reset was requested meanwhile
            return Task::none();
        }

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
        // in case usbsas failed to do it
        if let Some(false) = self.config.keep_tmp_files {
            let _ = fs::remove_file(&tar_path);
            let _ = fs::remove_file(&clean_tar_path);
            let _ = fs::remove_file(&fs_path);
        } else {
            if let Ok(metadata) = fs::metadata(&fs_path)
                && metadata.len() == 0
                && let Err(err) = fs::remove_file(Path::new(&fs_path))
            {
                log::error!("couldn't rm file {}: {err}", fs_path);
            };

            for path in &[&tar_path, &clean_tar_path] {
                if let Ok(metadata) = fs::metadata(path) {
                    // Empty tar with only data/ dir
                    if metadata.len() == 512
                        && let Err(err) = fs::remove_file(Path::new(&path))
                    {
                        log::error!("couldn't rm file {}: {err}", path);
                    };
                };
            }
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
        unsafe { std::env::set_var("USBSAS_SESSION_ID", &self.session_id) };

        Task::none()
    }

    /// Clear device selection without ending the session.
    fn soft_reset(&mut self) {
        self.src_id = None;
        self.dst_id = None;
        self.download_pin = None;
    }
}
