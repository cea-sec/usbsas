use usbsas_utils::{self, clap::UsbsasClap};

fn main() -> usbsas_net::Result<()> {
    usbsas_utils::log::init_logger();
    let matches = usbsas_utils::clap::new_usbsas_cmd("usbsas-uploader")
        .add_tar_path_arg()
        .get_matches();
    let tar_path = matches.get_one::<String>("tar_path").unwrap().to_owned();

    log::debug!("start ({}): {}", std::process::id(), tar_path);
    usbsas_net::Uploader::new(usbsas_comm::Comm::from_env()?, tar_path)?
        .main_loop()
        .map(|_| log::debug!("exit"))
}
