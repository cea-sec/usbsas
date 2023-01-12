fn main() -> usbsas_dev2scsi::Result<()> {
    usbsas_utils::log::init_logger();
    log::info!("start ({})", std::process::id());
    #[cfg(not(feature = "mock"))]
    assert!(rusb::supports_detach_kernel_driver());
    usbsas_dev2scsi::Dev2Scsi::new(
        usbsas_comm::Comm::from_env()?,
        #[cfg(not(feature = "mock"))]
        rusb::Context::new()?,
        #[cfg(feature = "mock")]
        usbsas_mock::mass_storage::MockContext {},
    )?
    .main_loop()
    .map(|_| log::debug!("exit"))
}
