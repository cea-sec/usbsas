fn main() -> usbsas_net::Result<()> {
    usbsas_utils::log::init_logger();
    log::info!("start ({})", std::process::id());
    usbsas_net::JsonParser::new(usbsas_comm::Comm::from_env()?)?
        .main_loop()
        .map(|_| log::debug!("exit"))
}
