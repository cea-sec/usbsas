//! Tool to test usbsas's upload / download / analyze features with a remote server.

use clap::{Arg, Command};
use std::io::{self, Write};
use thiserror::Error;
use usbsas_comm::{
    ComRqAnalyzer, ComRqDownloader, ComRqUploader, ProtoReqAnalyzer, ProtoReqCommon,
    ProtoReqDownloader, ProtoReqUploader,
};
use usbsas_config::{conf_parse, conf_read};
use usbsas_process::{ChildMngt, UsbsasChildSpawner};
use usbsas_proto as proto;

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("process: {0}")]
    Process(#[from] usbsas_process::Error),
    #[error("configuration or argument error: {0}")]
    ArgConf(String),
}
type Result<T> = std::result::Result<T, Error>;

fn upload(config_path: &str, bundle_path: &str, id: &str) -> Result<()> {
    let mut uploader = UsbsasChildSpawner::new("usbsas-uploader")
        .arg(bundle_path)
        .wait_on_startup()
        .spawn::<ComRqUploader>()?;

    let config = conf_parse(&conf_read(config_path)?)?;

    let networks = &config
        .networks
        .ok_or_else(|| Error::ArgConf("No networks".into()))?;

    let network = if networks.len() == 1 {
        &networks[0]
    } else {
        eprintln!("Multiple networks available, which one to upload to ?");
        for (index, net) in networks.iter().enumerate() {
            eprintln!("{}: {:?}", index + 1, net);
        }
        let n = loop {
            eprint!("[1-{}]: ", networks.len());
            io::stdout().flush().expect("couldn't flush stdout");
            let mut input = String::new();
            match io::stdin().read_line(&mut input) {
                Ok(_) => match input.trim().parse::<usize>() {
                    Ok(n) => {
                        if n > 0 && n <= networks.len() {
                            break n - 1;
                        } else {
                            log::error!("Index out of range");
                        }
                    }
                    Err(err) => log::error!("Couldn't parse input: {err}"),
                },
                Err(err) => log::error!("Couldn't read input: {err}"),
            }
        };
        &networks[n]
    };

    uploader.unlock_with(1)?;
    log::info!("Uploading bundle");
    uploader.comm.upload(proto::uploader::RequestUpload {
        id: id.to_string(),
        network: Some((&network.clone()).into()),
    })?;

    loop {
        let status = uploader.comm.recv_status()?;
        log::debug!("status: {}/{}", status.current, status.total);
        if status.done {
            break;
        }
    }

    if let Err(err) = uploader.comm.end() {
        log::error!("Couldn't end uploader: {err}");
    };

    log::info!("Bundle successfully uploaded");

    Ok(())
}

fn analyze(config_path: &str, bundle_path: &str, id: &str) -> Result<()> {
    let mut analyzer = UsbsasChildSpawner::new("usbsas-analyzer")
        .arg(bundle_path)
        .args(&["-c", config_path])
        .spawn::<ComRqAnalyzer>()?;

    analyzer
        .comm
        .analyze(proto::analyzer::RequestAnalyze { id: id.to_string() })?;

    loop {
        let status = analyzer.comm.recv_status()?;
        log::debug!("status: {}/{}", status.current, status.total);
        if status.done {
            break;
        }
    }

    let report = analyzer
        .comm
        .report(proto::analyzer::RequestReport {})?
        .report
        .expect("not report returned");

    log::info!("{report:?}");

    if let Err(err) = analyzer.comm.end() {
        log::error!("Couldn't end analyzer: {err}");
    };

    Ok(())
}

fn download(config_path: &str, bundle_path: &str, id: &str) -> Result<()> {
    let mut downloader = UsbsasChildSpawner::new("usbsas-downloader")
        .arg(bundle_path)
        .args(&["-c", config_path])
        .spawn::<ComRqDownloader>()?;

    let _ = downloader
        .comm
        .archiveinfos(proto::downloader::RequestArchiveInfos {
            path: format!("{id}/{bundle_path}"),
        })?
        .size;

    log::info!("Downloading bundle");
    downloader
        .comm
        .download(proto::downloader::RequestDownload {})?;

    loop {
        let status = downloader.comm.recv_status()?;
        log::debug!("status: {}/{}", status.current, status.total);
        if status.done {
            break;
        }
    }

    if let Err(err) = downloader.comm.end() {
        log::error!("Couldn't end downloader: {err}");
    };

    log::info!("Bundle successfully downloaded");

    Ok(())
}

fn main() -> Result<()> {
    env_logger::init_from_env(env_logger::Env::default().filter_or("RUST_LOG", "info"));
    let command = Command::new("usbsas-net")
        .about("Test uploading, downloading or analyzing a usbsas bundle")
        .version("1.0")
        .arg(
            clap::Arg::new("config")
                .short('c')
                .long("config")
                .help("Path of the configuration file")
                .num_args(1)
                .default_value(usbsas_utils::USBSAS_CONFIG)
                .required(false),
        )
        .arg(
            Arg::new("action")
                .value_name("ACTION")
                .index(1)
                .help("Action to perform: upload, analyze or download")
                .num_args(1)
                .required(true),
        )
        .arg(
            Arg::new("bundle")
                .value_name("FILE")
                .index(2)
                .help("Bundle to upload or test")
                .num_args(1)
                .required(true),
        )
        .arg(
            Arg::new("ID")
                .index(3)
                .help("ID of the user")
                .num_args(1)
                .required(true),
        );

    let matches = command.get_matches();
    let config_path = matches.get_one::<String>("config").unwrap();

    match (
        matches.get_one::<String>("action"),
        matches.get_one::<String>("bundle"),
        matches.get_one::<String>("ID"),
    ) {
        (Some(action), Some(path), Some(id)) => match action.as_str() {
            "upload" => upload(config_path, path, id)?,
            "analyze" => analyze(config_path, path, id)?,
            "download" => download(config_path, path, id)?,
            _ => {
                return Err(Error::ArgConf(
                    "Bad action specified, either: upload, analyze or download".to_owned(),
                ))
            }
        },
        _ => return Err(Error::ArgConf("args parse failed".to_owned())),
    }

    Ok(())
}
