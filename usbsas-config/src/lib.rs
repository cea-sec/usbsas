//! Parse usbsas toml configuration file. `conf_read()` and `conf_parse()` are
//! separated so that processes can read the file, enter `seccomp` and parse it
//! after.

use lazy_static::lazy_static;
use serde::Deserialize;
use std::{fs, io};

// Default environment variables to keep when forking
lazy_static! {
    static ref DEFAULT_ENV_VARS: Vec<String> = {
        vec![
            "TERM",
            "LANG",
            "KRB5CCNAME",
            "PATH",
            "RUST_LOG",
            "RUST_BACKTRACE",
            "USBSAS_MOCK_IN_DEV",
            "USBSAS_MOCK_OUT_DEV",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    };
}

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
}

#[derive(Debug, Deserialize)]
pub struct PathFilter {
    pub filters: Option<Vec<Filter>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Analyzer {
    pub url: String,
    pub krb_service_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UsbPortAccesses {
    pub ports_src: Vec<u8>,
    pub ports_dst: Vec<u8>,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub out_directory: String,
    pub env_vars: Option<Vec<String>>,
    pub message: Option<String>,
    pub command: Option<Command>,
    pub networks: Option<Vec<Network>>,
    pub filters: Option<Vec<Filter>>,
    pub post_copy: Option<PostCopy>,
    pub analyzer: Option<Analyzer>,
    pub usb_port_accesses: Option<UsbPortAccesses>,
}

impl Config {
    pub fn env_vars(&self) -> impl Iterator<Item = &str> {
        self.env_vars
            .as_ref()
            .unwrap_or(&DEFAULT_ENV_VARS)
            .iter()
            .map(|s| s.as_str())
    }
}

pub fn conf_read(config_path: &str) -> io::Result<String> {
    log::debug!("read config file: {}", config_path);
    fs::read_to_string(config_path)
}

pub fn conf_parse(conf_str: &str) -> io::Result<Config> {
    toml::from_str(conf_str).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Couldn't parse config: {}", err),
        )
    })
}
