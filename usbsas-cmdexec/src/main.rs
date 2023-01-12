use usbsas_utils::{self, clap::UsbsasClap};

fn main() -> usbsas_cmdexec::Result<()> {
    usbsas_utils::log::init_logger();

    let matches = usbsas_utils::clap::new_usbsas_cmd("usbsas-cmdexec")
        .add_tar_path_arg()
        .add_fs_path_arg()
        .add_config_arg()
        .get_matches();
    let config = matches.get_one::<String>("config").unwrap().to_owned();
    let tar_path = matches.get_one::<String>("tar_path").unwrap().to_owned();
    let fs_path = matches.get_one::<String>("fs_path").unwrap().to_owned();

    log::info!("start ({}): {} {}", std::process::id(), tar_path, fs_path);
    usbsas_cmdexec::CmdExec::new(usbsas_comm::Comm::from_env()?, tar_path, fs_path, config)?
        .main_loop()
        .map(|_| log::debug!("exit"))
}
