//! Parse usbsas toml configuration file. `conf_read()` and `conf_parse()` are
//! separated so that processes can read the file, enter `seccomp` and parse it
//! after.
//!
//! See `doc/configuration.md` for the description of each option.

use serde::Deserialize;
use std::{fs, io};

/// Remote server, destination (`[[networks]]`) or source (`[source_network]`).
#[derive(Clone, Debug, Deserialize)]
pub struct Network {
    /// Short name displayed to the user
    pub description: String,
    /// Long description displayed to the user
    pub longdescr: String,
    pub url: String,
    /// Kerberos service name (e.g. `HTTP@your.domain`), if Kerberos
    /// authentication is used
    pub krb_service_name: Option<String>,
}

/// Command destination (`[command]`), executed on the transfer tar.
#[derive(Clone, Debug, Deserialize)]
pub struct Command {
    /// Short name displayed to the user
    pub description: String,
    /// Long description displayed to the user
    pub longdescr: String,
    /// Path of the executable
    pub command_bin: String,
    /// Arguments, `%SOURCE_FILE%` is replaced by the path of the tar
    pub command_args: Vec<String>,
}

/// Local directory source (`[[src_local_dirs]]`).
#[derive(Clone, Debug, Deserialize)]
pub struct LocalDir {
    pub path: String,
    /// Short name displayed to the user
    pub description: String,
    /// Long description displayed to the user
    pub longdescr: String,
}

/// Command executed after a transfer (`[post_copy]`).
#[derive(Debug, Deserialize)]
pub struct PostCopy {
    pub description: String,
    /// Path of the executable
    pub command_bin: String,
    /// Arguments, `%SOURCE_FILE%` is replaced by the path of the output (tar or
    /// file system image)
    pub command_args: Vec<String>,
}

/// File name filter (`[[filters]]`). A file matching a filter is not
/// transferred. A filter matches if all of its set conditions match, they're
/// tested case insensitively on the full path of the file.
#[derive(Debug, Deserialize)]
pub struct Filter {
    /// Path contains all these strings
    pub contain: Option<Vec<String>>,
    /// Path starts with this string
    pub start: Option<String>,
    /// Path ends with this string
    pub end: Option<String>,
    /// Path is exactly this string
    pub exact: Option<String>,
}

/// List of filters. Unused.
#[derive(Debug, Deserialize)]
pub struct PathFilter {
    pub filters: Option<Vec<Filter>>,
}

/// Remote analyzer (`[analyzer]`). Only files from USB sources are analyzed.
#[derive(Clone, Debug, Deserialize)]
pub struct Analyzer {
    pub url: String,
    /// Kerberos service name, if Kerberos authentication is used
    pub krb_service_name: Option<String>,
    /// Analyze files transferred to a USB device
    pub analyze_usb: bool,
    /// Analyze files transferred to a network
    pub analyze_net: bool,
    /// Analyze files transferred to the command
    pub analyze_cmd: bool,
}

/// Restrict devices to USB ports (`[usb_port_accesses]`). A port is a list of
/// numbers: the bus, then the port number at each level of hubs (e.g. `[3, 2,
/// 5]`: port 5 of the hub on port 2 of bus 3). If a list isn't set, every port
/// is allowed.
#[derive(Debug, Deserialize)]
pub struct UsbPortAccesses {
    /// Ports where source devices can be plugged
    pub ports_src: Option<Vec<Vec<u8>>>,
    /// Ports where destination devices can be plugged
    pub ports_dst: Option<Vec<Vec<u8>>>,
    /// Ports where HID devices can be plugged
    pub ports_hid: Option<Vec<Vec<u8>>>,
}

/// Transfer reports (`[report]`).
#[derive(Debug, Deserialize)]
pub struct Report {
    /// Write the report on the destination
    pub write_dest: bool,
    /// Directory where the client saves reports
    pub write_local: Option<String>,
}

/// usbsas configuration.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Directory of the session's working files (`/tmp/usbsas` by default)
    #[serde(default = "default_out_dir")]
    pub out_directory: String,
    pub report: Option<Report>,
    /// Unused
    pub message: Option<String>,
    /// Title displayed in the client menu ("USBSAS" by default)
    pub window_title: Option<String>,
    /// Command destination
    pub command: Option<Command>,
    /// Network destinations
    pub networks: Option<Vec<Network>>,
    /// Network source
    pub source_network: Option<Network>,
    /// Local directory sources
    pub src_local_dirs: Option<Vec<LocalDir>>,
    pub filters: Option<Vec<Filter>>,
    pub post_copy: Option<PostCopy>,
    pub analyzer: Option<Analyzer>,
    pub usb_port_accesses: Option<UsbPortAccesses>,
    /// Language of the client (`en` or `fr`)
    pub lang: Option<String>,
    /// Image displayed in the client menu
    pub menu_img: Option<String>,
    /// Keep the session's working files after the session (false by default)
    pub keep_tmp_files: Option<bool>,
    // filled by usbsas process
    /// Free space in `out_directory`, in bytes (set by usbsas at runtime)
    pub available_space: Option<u64>,
    /// Id of the session (set by usbsas at runtime)
    pub session_id: Option<String>,
}

fn default_out_dir() -> String {
    String::from("/tmp/usbsas")
}

/// Read the configuration file.
pub fn conf_read(config_path: &str) -> io::Result<String> {
    log::debug!("read config file: {config_path}");
    fs::read_to_string(config_path)
}

/// Parse the content of a configuration file.
pub fn conf_parse(conf_str: &str) -> io::Result<Config> {
    toml::from_str(conf_str).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Couldn't parse config: {err}"),
        )
    })
}
