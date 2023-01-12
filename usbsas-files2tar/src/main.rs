use byteorder::ReadBytesExt;
use usbsas_utils::{self, clap::UsbsasClap};

fn main() -> usbsas_files2tar::Result<()> {
    usbsas_utils::log::init_logger();
    let matches = usbsas_utils::clap::new_usbsas_cmd("usbsas-files2tar")
        .add_tar_path_arg()
        .get_matches();
    let tar_path = matches.get_one::<String>("tar_path").unwrap().to_owned();

    log::info!("start ({}): {}", std::process::id(), tar_path);
    let mut comm = usbsas_comm::Comm::from_env()?;
    match comm.read_u8()? {
        // 0: unlock to start writing files in a tar
        0 => usbsas_files2tar::Files2Tar::new(comm, tar_path)?.main_loop()?,
        // 1: unlock to exit value
        1 => usbsas_files2tar::Files2Tar::new_end(comm)?.main_loop()?,
        _ => return Err(usbsas_files2tar::Error::Error("Bad unlock value".into())),
    }
    log::debug!("exit");
    Ok(())
}
