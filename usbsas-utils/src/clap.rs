//! Command line arguments shared by usbsas binaries.

/// Add common arguments to a [`clap::Command`].
pub trait UsbsasClap {
    /// `-c, --config <CONFIG_PATH>`: configuration file, defaults to
    /// [`USBSAS_CONFIG`](crate::USBSAS_CONFIG).
    fn add_config_arg(self) -> Self;
    /// `<FS_PATH>` (positional, required): output file system image.
    fn add_fs_path_arg(self) -> Self;
    /// `<TAR_PATH>` (positional, required): output tar archive.
    fn add_tar_path_arg(self) -> Self;
    /// `-s, --socket [SOCKET_DIR]`: use a Unix socket instead of pipes,
    /// created in `SOCKET_DIR` ([`DEFAULT_SOCKET_DIR`](crate::DEFAULT_SOCKET_DIR)
    /// if no value is given).
    fn add_socket_arg(self) -> Self;
}

impl UsbsasClap for clap::Command {
    fn add_config_arg(self) -> Self {
        self.arg(
            clap::Arg::new("config")
                .short('c')
                .long("config")
                .value_name("CONFIG_PATH")
                .help("Path of the configuration file")
                .num_args(1)
                .default_value(crate::USBSAS_CONFIG)
                .required(false),
        )
    }

    fn add_tar_path_arg(self) -> Self {
        self.arg(
            clap::Arg::new("tar_path")
                .value_name("TAR_PATH")
                .help("Output tar filename")
                .num_args(1)
                .required(true),
        )
    }

    fn add_fs_path_arg(self) -> Self {
        self.arg(
            clap::Arg::new("fs_path")
                .value_name("FS_PATH")
                .help("Output fs filename")
                .num_args(1)
                .required(true),
        )
    }

    fn add_socket_arg(self) -> Self {
        self.arg(
            clap::Arg::new("socket")
                .short('s')
                .long("socket")
                .value_name("SOCKET_DIR")
                .help("Use Unix domain socket to communicate (instead of pipes), optionally set directory where to create it")
                .default_missing_value(crate::DEFAULT_SOCKET_DIR)
                .num_args(0..=1)
                .required(false),
        )
    }
}

/// New command, to be completed with [`UsbsasClap`] methods.
pub fn new_usbsas_cmd(name: impl Into<clap::builder::Str>) -> clap::Command {
    clap::Command::new(name)
}
