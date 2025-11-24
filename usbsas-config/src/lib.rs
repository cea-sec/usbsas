//! Parse usbsas toml configuration file. `conf_read()` and `conf_parse()` are
//! separated so that processes can read the file, enter `seccomp` and parse it
//! after.

use serde::Deserialize;
use std::{fs, io};

#[derive(Clone, Debug, Deserialize)]
pub struct Network {
    pub description: String,
    pub longdescr: String,
    pub url: String,
    pub krb_service_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Command {
    pub description: String,
    pub longdescr: String,
    pub command_bin: String,
    pub command_args: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PostCopy {
    pub description: String,
    pub command_bin: String,
    pub command_args: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Filter {
    pub contain: Option<Vec<String>>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub exact: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PathFilter {
    pub filters: Option<Vec<Filter>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Analyzer {
    pub url: String,
    pub krb_service_name: Option<String>,
    pub analyze_usb: bool,
    pub analyze_net: bool,
    pub analyze_cmd: bool,
}

#[derive(Debug, Deserialize)]
pub struct UsbPortAccesses {
    pub ports_src: Vec<Vec<u8>>,
    pub ports_dst: Vec<Vec<u8>>,
}

#[derive(Debug, Deserialize)]
pub struct Report {
    pub write_dest: bool,
    pub write_local: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_out_dir")]
    pub out_directory: String,
    pub report: Option<Report>,
    pub message: Option<String>,
    pub window_title: Option<String>,
    pub command: Option<Command>,
    pub networks: Option<Vec<Network>>,
    pub source_network: Option<Network>,
    pub filters: Option<Vec<Filter>>,
    pub post_copy: Option<PostCopy>,
    pub analyzer: Option<Analyzer>,
    pub usb_port_accesses: Option<UsbPortAccesses>,
    pub lang: Option<String>,
    pub menu_img: Option<String>,
    pub keep_tmp_files: Option<bool>,
    // filled by usbsas process
    pub available_space: Option<u64>,
}

fn default_out_dir() -> String {
    String::from("/tmp/usbsas")
}

pub fn conf_read(config_path: &str) -> io::Result<String> {
    log::debug!("read config file: {config_path}");
    fs::read_to_string(config_path)
}

pub fn conf_parse(conf_str: &str) -> io::Result<Config> {
    toml::from_str(conf_str).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Couldn't parse config: {err}"),
        )
    })
}
