use usbsas_utils::{self, clap::UsbsasClap};

fn main() -> usbsas_local2files::Result<()> {
    usbsas_utils::log::init_logger();

    let matches = usbsas_utils::clap::new_usbsas_cmd("usbsas-local2files")
        .add_config_arg()
        .get_matches();
    let config_path = matches.get_one::<String>("config").unwrap().to_owned();

    log::info!("start ({})", std::process::id());
    usbsas_local2files::Local2Files::new(usbsas_comm::Comm::from_env()?, config_path)?
        .main_loop()
        .map(|_| log::debug!("exit"))
}
