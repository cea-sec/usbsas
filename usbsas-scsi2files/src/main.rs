fn main() -> usbsas_scsi2files::Result<()> {
    usbsas_utils::log::init_logger();

    log::info!("start ({})", std::process::id());
    usbsas_scsi2files::Scsi2Files::new(usbsas_comm::Comm::from_env()?)?
        .main_loop()
        .map(|_| log::debug!("exit"))
}
