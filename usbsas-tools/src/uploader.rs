//! Tool to test usbsas's upload / analyze features with a remote server.

use clap::{Arg, Command};
use thiserror::Error;
use usbsas_comm::{protorequest, Comm};
use usbsas_process::UsbsasChildSpawner;
use usbsas_proto as proto;

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("privileges: {0}")]
    Privileges(#[from] usbsas_privileges::Error),
    #[error("privileges: {0}")]
    Process(#[from] usbsas_process::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("upload error: {0}")]
    Upload(String),
    #[error("analyze error: {0}")]
    Analyze(String),
}
type Result<T> = std::result::Result<T, Error>;

protorequest!(
    CommUploader,
    uploader,
    upload = Upload[RequestUpload, ResponseUpload],
    end = End[RequestEnd, ResponseEnd]
);

protorequest!(
    CommAnalyzer,
    analyzer,
    analyze = Analyze[RequestAnalyze, ResponseAnalyze],
    end = End[RequestEnd, ResponseEnd]
);

fn upload(config_path: &str, bundle_path: &str, id: &str) -> Result<()> {
    use proto::uploader::response::Msg;
    let mut uploader = UsbsasChildSpawner::new()
        .arg(bundle_path)
        .arg(config_path)
        .spawn::<usbsas_net::Uploader, proto::uploader::Request>()?;

    log::info!("Uploading bundle");
    uploader.comm.send(proto::uploader::Request {
        msg: Some(proto::uploader::request::Msg::Upload(
            proto::uploader::RequestUpload { id: id.to_string() },
        )),
    })?;

    loop {
        let rep: proto::uploader::Response = uploader.comm.recv()?;
        match rep.msg.ok_or(Error::BadRequest)? {
            Msg::UploadStatus(status) => {
                log::debug!("status: {}/{}", status.current_size, status.total_size);
                continue;
            }
            Msg::Upload(_) => {
                break;
            }
            Msg::Error(err) => {
                log::error!("Upload error: {:?}", err);
                return Err(Error::Upload(err.err));
            }
            _ => {
                log::error!("bad resp");
                return Err(Error::BadRequest);
            }
        }
    }

    if let Err(err) = uploader.comm.end(proto::uploader::RequestEnd {}) {
        log::error!("Couldn't end uploader: {}", err);
    };

    log::info!("Bundle successfully uploaded");

    Ok(())
}

fn analyze(config_path: &str, bundle_path: &str, id: &str) -> Result<()> {
    use proto::analyzer::response::Msg;
    let mut analyzer = UsbsasChildSpawner::new()
        .arg(bundle_path)
        .arg(config_path)
        .spawn::<usbsas_net::Analyzer, proto::analyzer::Request>()?;

    analyzer.comm.send(proto::analyzer::Request {
        msg: Some(proto::analyzer::request::Msg::Analyze(
            proto::analyzer::RequestAnalyze { id: id.to_string() },
        )),
    })?;

    loop {
        let rep: proto::analyzer::Response = analyzer.comm.recv()?;
        match rep.msg.ok_or(Error::BadRequest)? {
            Msg::Analyze(res) => {
                log::info!(
                    "Analyzer status: clean: {:#?}, dirty: {:#?}",
                    &res.clean,
                    &res.dirty
                );
                break;
            }
            Msg::UploadStatus(status) => {
                log::debug!("status: {}/{}", status.current_size, status.total_size);
                continue;
            }
            Msg::Error(err) => {
                log::error!("{}", err.err);
                return Err(Error::Analyze(err.err));
            }
            _ => return Err(Error::Analyze("Unexpected response".into())),
        }
    }

    if let Err(err) = analyzer.comm.end(proto::analyzer::RequestEnd {}) {
        log::error!("Couldn't end analyzer: {}", err);
    };

    Ok(())
}

fn main() -> Result<()> {
    env_logger::init_from_env(env_logger::Env::default().filter_or("RUST_LOG", "info"));
    let command = Command::new("usbsas-uploader")
        .about("Test uploading or analyzing a usbsas bundle")
        .version("1.0")
        .arg(
            clap::Arg::new("config")
                .short('c')
                .long("config")
                .help("Path of the configuration file")
                .takes_value(true)
                .default_value(usbsas_utils::USBSAS_CONFIG)
                .required(false),
        )
        .arg(
            Arg::new("bundle")
                .value_name("FILE")
                .index(1)
                .help("Bundle to upload or test")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::new("ID")
                .index(2)
                .help("ID of the user")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::new("analyze")
                .short('a')
                .long("analyze")
                .help("Analyze instead of upload"),
        );

    let matches = command.get_matches();
    let config_path = matches.get_one::<String>("config").unwrap();

    if let Some(path) = matches.get_one::<String>("bundle") {
        if let Some(id) = matches.get_one::<String>("ID") {
            if matches.contains_id("analyze") {
                analyze(config_path, path, id)?;
                return Ok(());
            }
            upload(config_path, path, id)?;
        }
    }

    Ok(())
}
