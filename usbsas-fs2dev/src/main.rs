use usbsas_utils::{self, clap::UsbsasClap};

fn main() -> usbsas_fs2dev::Result<()> {
    usbsas_utils::log::init_logger();
    let matches = usbsas_utils::clap::new_usbsas_cmd("usbsas-fs2dev")
        .add_fs_path_arg()
        .get_matches();
    let fs_path = matches.get_one::<String>("fs_path").unwrap().to_owned();

    log::info!("start ({}): {}", std::process::id(), fs_path);
    usbsas_fs2dev::Fs2Dev::new(usbsas_comm::Comm::from_env()?, fs_path)?
        .main_loop()
        .map(|_| log::debug!("exit"))
}
