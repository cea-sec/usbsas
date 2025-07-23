//! Make an image of a USB mass storage device (like dd) with usbsas.

use clap::{Arg, Command};
use std::{
    fs,
    io::{self, Write},
    os::unix::io::AsRawFd,
    path,
};
use thiserror::Error;
use usbsas_comm::{ComRqScsi, ProtoReqScsi, SendRecv, ToFd};
use usbsas_config::{conf_parse, conf_read};
use usbsas_process::{ChildMngt, UsbsasChild, UsbsasChildSpawner};
use usbsas_proto as proto;
use usbsas_utils::READ_FILE_MAX_SIZE;

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("{0}")]
    Arg(String),
    #[error("{0}")]
    OpenDevice(String),
    #[error("persist error: {0}")]
    Persist(#[from] tempfile::PersistError),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("process: {0}")]
    Process(#[from] usbsas_process::Error),
    #[error("progress error: {0}")]
    Progress(#[from] indicatif::style::TemplateError),
}
type Result<T> = std::result::Result<T, Error>;

struct Imager {
    dev2scsi: UsbsasChild<ComRqScsi>,
    writer: Box<dyn Write>,
    busnum: u32,
    devnum: u32,
}

impl Imager {
    fn new(out_file: Option<fs::File>, busnum: u32, devnum: u32) -> Result<Self> {
        let mut pipes_read = vec![];
        let mut pipes_write = vec![];

        log::debug!("spawn dev2scsi ");
        let dev2scsi = UsbsasChildSpawner::new("usbsas-dev2scsi")
            .wait_on_startup()
            .spawn::<ComRqScsi>()?;
        pipes_read.push(dev2scsi.comm.input_fd());
        pipes_write.push(dev2scsi.comm.output_fd());

        let writer: Box<dyn Write> = if let Some(file) = out_file {
            pipes_write.push(file.as_raw_fd());
            Box::new(file)
        } else {
            pipes_write.push(1);
            Box::new(io::stdout())
        };

        usbsas_sandbox::imager::seccomp(pipes_read, pipes_write)?;

        Ok(Imager {
            dev2scsi,
            writer,
            busnum,
            devnum,
        })
    }

    fn image_device(&mut self) -> Result<()> {
        // Unlock dev2scsi
        self.dev2scsi
            .unlock_with((u64::from(self.devnum) << 32) | u64::from(self.busnum))?;

        let rep: proto::scsi::Response = self.dev2scsi.comm.recv()?;
        let (dev_size, block_size) =
            if let Some(proto::scsi::response::Msg::OpenDevice(rep)) = rep.msg {
                (rep.dev_size, rep.block_size)
            } else {
                return Err(Error::OpenDevice("Couldn't open device".to_string()));
            };

        let mut todo = dev_size;
        let mut sector_count: u64 = READ_FILE_MAX_SIZE / block_size;
        let mut offset: u64 = 0;

        // Shiny progress bar
        let pb = indicatif::ProgressBar::new(dev_size);
        pb.set_style(
            indicatif::ProgressStyle::default_bar()
                .template("[{wide_bar}] {bytes}/{total_bytes} ({eta})")?
                // .map_err(|err| Error::Progress(format!("progress bar err: {err}")))?
                .progress_chars("#>-"),
        );

        while todo != 0 {
            if todo < READ_FILE_MAX_SIZE {
                sector_count = todo / block_size;
            }
            let rep = self
                .dev2scsi
                .comm
                .readsectors(proto::scsi::RequestReadSectors {
                    offset,
                    count: sector_count,
                })?;

            self.writer.write_all(&rep.data)?;
            self.writer.flush()?;

            offset += sector_count;
            todo -= sector_count * block_size;

            pb.set_position(offset * block_size);

            // log::debug!(
            //     "{: >3}% ({: >11} B / {: >11} B)",
            //     (offset * block_size) as u64 * 100 / dev_size,
            //     (offset * block_size) as u64,
            //     dev_size
            // );
        }
        drop(pb);

        Ok(())
    }
}

impl Drop for Imager {
    // Properly end children
    fn drop(&mut self) {
        log::debug!("End children");
        self.dev2scsi.end().expect("Couldn't end dev2scsi");
    }
}

fn main() -> Result<()> {
    env_logger::builder().format_timestamp(None).init();
    let matches = Command::new("usbsas-imager")
        .about("Clone a usb device (Mass Storage) with usbsas")
        .version("1.0")
        .arg(
            Arg::new("busnum")
                .value_name("BUSNUM")
                .requires("devnum")
                .value_parser(clap::value_parser!(u32))
                .help("Bus number of the output device")
                .index(1)
                .num_args(1)
                .required(true),
        )
        .arg(
            Arg::new("devnum")
                .value_name("DEVNUM")
                .requires("busnum")
                .value_parser(clap::value_parser!(u32))
                .help("Device number of the output device")
                .index(2)
                .num_args(1)
                .required(true),
        )
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
            Arg::new("output")
                .short('o')
                .long("output")
                .value_name("FILE")
                .help("Path of the output file")
                .conflicts_with("stdout")
                .num_args(1),
        )
        .arg(
            Arg::new("stdout")
                .short('O')
                .long("stdout")
                .help("Output to stdout")
                .conflicts_with("output")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    let config_path = matches.get_one::<String>("config").unwrap();
    let writer = if let Some(path) = matches.get_one::<String>("output") {
        match fs::File::create(path) {
            Ok(file) => Some(file),
            Err(err) => {
                log::error!("Couldn't create file {path}: {err}");
                return Err(err.into());
            }
        }
    } else if matches.get_flag("stdout") {
        None
    } else {
        let config = conf_parse(&conf_read(config_path)?)?;
        let out_dir = path::Path::new(&config.out_directory);
        let (out_file, out_path) = tempfile::Builder::new()
            .prefix("device_image_")
            .suffix(".bin")
            .rand_bytes(6)
            .tempfile_in(out_dir)?
            .keep()?;
        eprintln!("Cloning to {}", out_path.display());
        Some(out_file)
    };

    let (busnum, devnum) = match (
        matches.get_one::<u32>("busnum"),
        matches.get_one::<u32>("devnum"),
    ) {
        (Some(busnum), Some(devnum)) => (busnum.to_owned(), devnum.to_owned()),
        _ => {
            return Err(Error::Arg(
                "Must specify both busnum and devnum".to_string(),
            ));
        }
    };

    let mut imager = Imager::new(writer, busnum, devnum)?;

    imager.image_device()?;

    eprintln!("Device cloned successfully");

    Ok(())
}
