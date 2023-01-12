#[cfg(feature = "mock")]
use usbsas_mock::usbdev::MockUsbDev as UsbDev;
#[cfg(not(feature = "mock"))]
use usbsas_usbdev::UsbDev;
use usbsas_utils::{self, clap::UsbsasClap};

fn main() -> usbsas_usbdev::Result<()> {
    usbsas_utils::log::init_logger();
    let matches = usbsas_utils::clap::new_usbsas_cmd("usbsas-usbdev")
        .add_config_arg()
        .get_matches();
    let config = matches.get_one::<String>("config").unwrap().to_owned();

    log::info!("start ({})", std::process::id());
    UsbDev::new(usbsas_comm::Comm::from_env()?, config)?.main_loop()?;
    log::debug!("exit");
    Ok(())
}
