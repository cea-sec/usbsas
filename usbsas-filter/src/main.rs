use usbsas_utils::{self, clap::UsbsasClap};

fn main() -> usbsas_filter::Result<()> {
    usbsas_utils::log::init_logger();
    let matches = usbsas_utils::clap::new_usbsas_cmd("usbsas-filter")
        .add_config_arg()
        .get_matches();
    let config = matches.get_one::<String>("config").unwrap().to_owned();

    log::info!("start ({})", std::process::id());
    usbsas_filter::Filter::new(usbsas_comm::Comm::from_env()?, config)?
        .main_loop()
        .map(|_| log::debug!("exit"))
}
