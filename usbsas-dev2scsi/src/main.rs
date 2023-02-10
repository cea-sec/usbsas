fn main() -> usbsas_dev2scsi::Result<()> {
    usbsas_utils::log::init_logger();
    log::info!("start ({})", std::process::id());
    usbsas_dev2scsi::Dev2Scsi::new(usbsas_comm::Comm::from_env()?)?
        .main_loop()
        .map(|_| log::debug!("exit"))
}
