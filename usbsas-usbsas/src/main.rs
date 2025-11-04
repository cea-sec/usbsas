use anyhow::{Context, Result};
use log::{error, info};
use std::{
    env,
    fs::{self, File, Permissions},
    os::{
        fd::AsRawFd,
        unix::{fs::PermissionsExt, net::UnixListener},
    },
};
use usbsas_comm::{ComRpUsbsas, Comm, ProtoRespUsbsas, ToFd};
use usbsas_config::{conf_parse, conf_read, Config};
use usbsas_usbsas::{
    children::Children,
    states::{EndState, InitState, State},
};
use usbsas_utils::clap::UsbsasClap;

pub struct UnixSocketPath {
    path: String,
}

impl Drop for UnixSocketPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn main_loop(
    mut comm: impl ProtoRespUsbsas,
    mut children: Children,
    config: Config,
    _socket_path: Option<UnixSocketPath>,
) -> Result<()> {
    let init_state = InitState {
        config,
        plugged_devices: Vec::new(),
    };
    let mut state = State::Init(init_state);
    loop {
        state = match state.run(&mut comm, &mut children) {
            Ok(State::Exit) => break,
            Ok(state) => state,
            Err(err) => {
                error!("{err}, waiting end");
                comm.error(&err)?;
                State::End(EndState { report: None })
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let matches = usbsas_utils::clap::new_usbsas_cmd("usbsas-usbsas")
        .add_config_arg()
        .add_socket_arg()
        .get_matches();

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
    info!("init {session_id} ({})", std::process::id());

    let config_path = matches.get_one::<String>("config").unwrap();
    let mut config = conf_parse(&conf_read(config_path)?)?;

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
    let children = Children::spawn(config_path, &tar_path, &fs_path).context("spawn children")?;

    // Get file descriptors to apply seccomp rules
    let mut pipes_read = vec![];
    let mut pipes_write = vec![];
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

    let fs_stats = nix::sys::statvfs::statvfs(config.out_directory.as_str())?;
    let available = fs_stats.block_size() * fs_stats.blocks_available();
    config.available_space = Some(available);

    if matches.contains_id("socket") {
        let socket_path = match matches.get_one::<String>("socket") {
            Some(path) => path.to_string(),
            None => format!(
                "{}/usbsas.sock",
                &config.out_directory.trim_end_matches('/')
            ),
        };
        if let Ok(true) = std::path::Path::new(&socket_path).try_exists() {
            log::warn!("socket already exists, probably residual, removing it");
            fs::remove_file(&socket_path).expect("remove socket");
        };
        let listener = UnixListener::bind(&socket_path).context("bind")?;
        // Set R+W for owner and group
        fs::set_permissions(&socket_path, Permissions::from_mode(0o660))
            .context("set perms socket")?;
        let stream = match listener.incoming().next() {
            Some(Ok(stream)) => stream,
            Some(Err(err)) => panic!("error listen incoming {err}"),
            None => panic!("shouldn't happen"),
        };
        let comm = Comm::new(stream.try_clone().context("clone stream")?, stream);
        let socket = usbsas_sandbox::usbsas::UsbsasSocket {
            listen: listener.as_raw_fd(),
            read: comm.input_fd(),
            write: comm.output_fd(),
            path: socket_path.clone(),
        };
        pipes_read.push(socket.read);
        pipes_write.push(socket.write);
        usbsas_sandbox::usbsas::sandbox(
            pipes_read,
            pipes_write,
            Some(socket),
            &config.out_directory,
        )
        .context("seccomp")?;
        main_loop(
            comm,
            children,
            config,
            Some(UnixSocketPath { path: socket_path }),
        )
        .context("main loop")
    } else {
        let comm: ComRpUsbsas = Comm::from_env()?;
        pipes_read.push(comm.input_fd());
        pipes_write.push(comm.output_fd());
        usbsas_sandbox::usbsas::sandbox(pipes_read, pipes_write, None, &config.out_directory)
            .context("seccomp")?;
        main_loop(comm, children, config, None).context("main loop")
    }
}
