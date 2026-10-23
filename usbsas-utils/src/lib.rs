//! usbsas constants and logging utility.
//!
//! Also provides command line helpers ([`clap`]) shared by usbsas binaries.

use std::env;

pub mod clap;
pub mod log;

/// Environment variable holding the file descriptor a child process reads its
/// requests from.
pub const INPUT_PIPE_FD_VAR: &str = "INPUT_PIPE_FD";
/// Environment variable holding the file descriptor a child process writes its
/// responses to.
pub const OUTPUT_PIPE_FD_VAR: &str = "OUTPUT_PIPE_FD";
/// Maximum size of a protobuf message, larger messages are rejected.
pub const MAX_PROTO_MESS_SIZE: u64 = 1024 * 1024 * 32;
/// Maximum size of data read (file content or sectors) in one request.
pub const READ_FILE_MAX_SIZE: u64 = 1024 * 1024 * 10;
/// Sector size of the file systems created by usbsas.
pub const SECTOR_SIZE: u64 = 512;
/// Block size of tar archives.
pub const TAR_BLOCK_SIZE: usize = 512;
/// Directory containing the transferred files in usbsas tar archives.
pub const TAR_DATA_DIR: &str = "data";
/// Default directory of the usbsas server socket.
pub const DEFAULT_SOCKET_DIR: &str = "/run/usbsas";
/// Directory of the usbsas executables, from the `USBSAS_BIN_PATH` environment
/// variable at build time (defaults to the cargo target directory).
pub const USBSAS_BIN_PATH: &str = env!("USBSAS_BIN_PATH");
/// Default configuration file, from the `USBSAS_CONFIG` environment variable at
/// build time (defaults to `/etc/usbsas/config.toml`).
pub const USBSAS_CONFIG: &str = match option_env!("USBSAS_CONFIG") {
    Some(val) => val,
    None => "/etc/usbsas/config.toml",
};

/// formats a byte array as an hexadecimal pretty string
#[macro_export]
macro_rules! formathex {
    ( $array:expr ) => {
        $array
            .iter()
            .map(|x| format!("{:02x} ", x))
            .collect::<String>()
    };
}

/// prints a byte array as an hexadecimal string (debug-only)
#[macro_export]
macro_rules! dbgprinthex {
    ( $array:expr ) => {
        if cfg!(debug_assertions) {
            println!("{}", formathex!($array));
        }
    };
}
