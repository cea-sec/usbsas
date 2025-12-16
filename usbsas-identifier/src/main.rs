fn main() -> usbsas_identifier::Result<()> {
    usbsas_utils::log::init_logger();
    log::info!("start ({})", std::process::id());
    usbsas_identifier::Identifier::new(usbsas_comm::Comm::from_env()?)?
        .main_loop()
        .map(|_| log::debug!("exit"))
}
