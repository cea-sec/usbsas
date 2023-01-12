pub trait UsbsasClap {
    fn add_config_arg(self) -> Self;
    fn add_fs_path_arg(self) -> Self;
    fn add_tar_path_arg(self) -> Self;
}

impl UsbsasClap for clap::Command {
    fn add_config_arg(self) -> Self {
        self.arg(
            clap::Arg::new("config")
                .short('c')
                .long("config")
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
}

pub fn new_usbsas_cmd(name: impl Into<clap::builder::Str>) -> clap::Command {
    clap::Command::new(name)
}
