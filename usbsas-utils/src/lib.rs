//! usbsas constants and logging utility.

use std::env;

pub mod clap;
pub mod log;

pub const INPUT_PIPE_FD_VAR: &str = "INPUT_PIPE_FD";
pub const OUTPUT_PIPE_FD_VAR: &str = "OUTPUT_PIPE_FD";
pub const READ_FILE_MAX_SIZE: u64 = 1024 * 1024 * 10;
pub const SECTOR_SIZE: u64 = 512;
pub const TAR_BLOCK_SIZE: usize = 512;
pub const TAR_DATA_DIR: &str = "data";
pub const USBSAS_BIN_PATH: &str = env!("USBSAS_BIN_PATH");
pub const USBSAS_CONFIG: &str = match option_env!("USBSAS_CONFIG") {
    Some(val) => val,
    None => "/etc/usbsas/config.toml",
};
pub const USBSAS_VERSION: &str = env!("USBSAS_VERSION");

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
