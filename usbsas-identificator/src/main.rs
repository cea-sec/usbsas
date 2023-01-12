fn main() -> usbsas_identificator::Result<()> {
    usbsas_utils::log::init_logger();
    log::info!("start ({})", std::process::id());
    usbsas_identificator::Identificator::new(usbsas_comm::Comm::from_env()?)?
        .main_loop()
        .map(|_| log::debug!("exit"))
}
