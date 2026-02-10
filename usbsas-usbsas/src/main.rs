use anyhow::{bail, Context, Result};
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
    TmpFiles,
};
use usbsas_utils::clap::UsbsasClap;

fn main_loop(
    mut comm: impl ProtoRespUsbsas,
    mut children: Children,
    config: Config,
    _tmp_files: TmpFiles,
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

    // Create out dir & temp files
    match fs::metadata(&config.out_directory) {
        Ok(md) => {
            if !md.is_dir() {
                bail!("out_directory '{}' is not a dir", &config.out_directory);
            }
        }
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                fs::create_dir_all(&config.out_directory)?;
            } else {
                return Err(err.into());
            }
        }
    }
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

    let mut tmpfiles = TmpFiles {
        tar_path,
        clean_tar_path,
        fs_path,
        socket_path: None,
        keep: config.keep_tmp_files.unwrap_or(false),
    };

    // Get file descriptors to apply seccomp rules
    let mut pipes_read = vec![];
    let mut pipes_write = vec![];
    let comms: [&dyn ToFd; 13] = [
        &children.analyzer.comm,
        &children.identifier.comm,
        &children.cmdexec.comm,
        &children.downloader.comm,
        &children.files2fs.comm,
        &children.files2tar.comm,
        &children.files2cleantar.comm,
        &children.local2files.comm,
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

    if let Some(dir) = matches.get_one::<String>("socket") {
        match fs::metadata(dir) {
            Ok(md) => {
                if !md.is_dir() {
                    bail!("socket dir '{}' is not a dir", &dir);
                }
            }
            Err(err) => {
                if err.kind() == std::io::ErrorKind::NotFound {
                    fs::create_dir_all(dir)?;
                } else {
                    return Err(err.into());
                }
            }
        }
        let socket_path = format!("{}/usbsas.sock", dir.trim_end_matches('/'));
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
        let paths_rm: Option<&[&str]> = if let Some(false) = config.keep_tmp_files {
            Some(&[config.out_directory.as_str(), dir])
        } else {
            Some(&[dir])
        };
        usbsas_sandbox::usbsas::sandbox(pipes_read, pipes_write, Some(socket), paths_rm)
            .context("seccomp")?;
        tmpfiles.socket_path = Some(socket_path);
        main_loop(comm, children, config, tmpfiles).context("main loop")
    } else {
        let comm: ComRpUsbsas = Comm::from_env()?;
        pipes_read.push(comm.input_fd());
        pipes_write.push(comm.output_fd());
        let paths_rm: Option<&[&str]> = if let Some(false) = config.keep_tmp_files {
            Some(&[config.out_directory.as_str()])
        } else {
            None
        };
        usbsas_sandbox::usbsas::sandbox(pipes_read, pipes_write, None, paths_rm)
            .context("seccomp")?;
        main_loop(comm, children, config, tmpfiles).context("main loop")
    }
}
