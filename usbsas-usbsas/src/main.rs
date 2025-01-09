use anyhow::{Context, Result};
use log::{error, info, trace};
use std::{env, fs::File};
use usbsas_comm::{ComRpUsbsas, Comm, ProtoRespCommon, ToFd};
use usbsas_config::{conf_parse, conf_read, Config};
use usbsas_usbsas::{
    children::Children,
    states::{EndState, InitState, RunState, State},
};
use usbsas_utils::clap::UsbsasClap;

fn main_loop(mut comm: ComRpUsbsas, mut children: Children, config: Config) -> Result<()> {
    let mut init_state = InitState {
        config,
        plugged_devices: Vec::new(),
    };
    let mut report = init_state.init_report()?;
    let mut state = State::Init(init_state);
    loop {
        state = match state.run(&mut comm, &mut children) {
            Ok(State::Exit) => break,
            Ok(state) => state,
            Err(err) => {
                error!("{}, waiting end", err);
                comm.error(&err)?;
                report["status"] = "error".into();
                report["reason"] = err.to_string().into();
                State::End(EndState {
                    report: report.clone(),
                })
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    // Create and set env session id if caller didn't
    let session_id = match env::var("USBSAS_SESSION_ID") {
        Ok(id) => id,
        Err(_) => {
            let id = uuid::Uuid::new_v4().simple().to_string();
            env::set_var("USBSAS_SESSION_ID", &id);
            id
        }
    };

    usbsas_utils::log::init_logger();
    info!("init {} ({})", session_id, std::process::id());

    let matches = usbsas_utils::clap::new_usbsas_cmd("usbsas-usbsas")
        .add_config_arg()
        .get_matches();
    let config_path = matches.get_one::<String>("config").unwrap();
    let config = conf_parse(&conf_read(config_path)?)?;

    // Create temp files
    let tar_path = format!(
        "{}/usbsas_{}.tar",
        &config.out_directory.trim_end_matches('/'),
        session_id
    );
    let _ = File::create(&tar_path).context(format!("create {tar_path}"))?;
    let clean_tar_path = format!(
        "{}/usbsas_{}_clean.tar",
        &config.out_directory.trim_end_matches('/'),
        session_id
    );
    let _ = File::create(&clean_tar_path).context(format!("create {clean_tar_path}"))?;
    let fs_path = format!(
        "{}/usbsas_{}.img",
        &config.out_directory.trim_end_matches('/'),
        session_id
    );
    let _ = File::create(&fs_path).context(format!("create {fs_path}"))?;

    // Spawn children
    let comm: ComRpUsbsas = Comm::from_env()?;
    let children = Children::spawn(config_path, &tar_path, &fs_path).context("spawn children")?;

    // Get file descriptors to apply seccomp rules
    let mut pipes_read = vec![];
    let mut pipes_write = vec![];
    pipes_read.push(comm.input_fd());
    pipes_write.push(comm.output_fd());
    let comms: [&dyn ToFd; 12] = [
        &children.analyzer.comm,
        &children.identificator.comm,
        &children.cmdexec.comm,
        &children.downloader.comm,
        &children.files2fs.comm,
        &children.files2tar.comm,
        &children.files2cleantar.comm,
        &children.fs2dev.comm,
        &children.scsi2files.comm,
        &children.tar2files.comm,
        &children.uploader.comm,
        &children.usbdev.comm,
    ];
    comms.iter().for_each(|c| {
        pipes_read.push(c.input_fd());
        pipes_write.push(c.output_fd())
    });
    trace!("enter seccomp");
    usbsas_sandbox::usbsas::seccomp(pipes_read, pipes_write).context("seccomp")?;

    main_loop(comm, children, config).context("main loop")
}
