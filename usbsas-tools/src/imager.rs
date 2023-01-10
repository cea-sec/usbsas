//! Make an image of a USB mass storage device (like dd) with usbsas.

use clap::{Arg, Command};
use std::{
    fs,
    io::{self, Write},
    os::unix::io::AsRawFd,
    path,
};
use thiserror::Error;
use usbsas_comm::{protorequest, Comm};
use usbsas_config::{conf_parse, conf_read};
use usbsas_process::{UsbsasChild, UsbsasChildSpawner};
use usbsas_proto as proto;
use usbsas_proto::common::Device;
use usbsas_utils::READ_FILE_MAX_SIZE;

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("int error: {0}")]
    Tryfromint(#[from] std::num::TryFromIntError),
    #[error("{0}")]
    Error(String),
    #[error("persist error: {0}")]
    Persist(#[from] tempfile::PersistError),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("process: {0}")]
    Process(#[from] usbsas_process::Error),
}
type Result<T> = std::result::Result<T, Error>;

protorequest!(
    CommUsbdev,
    usbdev,
    devices = Devices[RequestDevices, ResponseDevices],
    end = End[RequestEnd, ResponseEnd]
);

protorequest!(
    CommScsi,
    scsi,
    partitions = Partitions[RequestPartitions, ResponsePartitions],
    readsectors = ReadSectors[RequestReadSectors, ResponseReadSectors],
    end = End[RequestEnd, ResponseEnd],
    opendev = OpenDevice[RequestOpenDevice, ResponseOpenDevice]
);

protorequest!(
    CommWritefs,
    writefs,
    setfsinfos = SetFsInfos[RequestSetFsInfos, ResponseSetFsInfos],
    newfile = NewFile[RequestNewFile, ResponseNewFile],
    writefile = WriteFile[RequestWriteFile, ResponseWriteFile],
    endfile = EndFile[RequestEndFile, ResponseEndFile],
    close = Close[RequestClose, ResponseClose],
    bitvec = BitVec[RequestBitVec, ResponseBitVec],
    imgdisk = ImgDisk[RequestImgDisk, ResponseImgDisk],
    writedata = WriteData[RequestWriteData, ResponseWriteData],
    end = End[RequestEnd, ResponseEnd]
);

struct BusDevNum {
    busnum: u32,
    devnum: u32,
}

struct Imager {
    usbdev: Option<UsbsasChild<proto::usbdev::Request>>,
    dev2scsi: UsbsasChild<proto::scsi::Request>,
    writer: Box<dyn Write>,
    busdevnum: Option<BusDevNum>,
}

impl Imager {
    fn new(
        config_path: &str,
        out_file: Option<fs::File>,
        busdevnum: Option<BusDevNum>,
    ) -> Result<Self> {
        let mut pipes_read = vec![];
        let mut pipes_write = vec![];

        log::debug!("Starting usbsas children");
        let dev2scsi = UsbsasChildSpawner::new()
            .wait_on_startup()
            .spawn::<usbsas_dev2scsi::Dev2Scsi, proto::scsi::Request>()?;
        pipes_read.push(dev2scsi.comm.input_fd());
        pipes_write.push(dev2scsi.comm.output_fd());

        let writer: Box<dyn Write> = if let Some(file) = out_file {
            pipes_write.push(file.as_raw_fd());
            Box::new(file)
        } else {
            pipes_write.push(1);
            Box::new(io::stdout())
        };

        // If busnum and devnum were not specified we need usbdev to select the device
        let usbdev = if busdevnum.is_none() {
            let usbdev = UsbsasChildSpawner::new()
                .arg(config_path)
                .spawn::<usbsas_usbdev::UsbDev, proto::usbdev::Request>()?;
            pipes_read.push(usbdev.comm.input_fd());
            pipes_write.push(usbdev.comm.output_fd());
            Some(usbdev)
        } else {
            None
        };

        usbsas_sandbox::imager::seccomp(pipes_read, pipes_write)?;

        Ok(Imager {
            usbdev,
            dev2scsi,
            writer,
            busdevnum,
        })
    }

    fn list_devices(&mut self) -> Result<Vec<Device>> {
        log::debug!("Listing usb devices (mass storage)");
        let mut usbdev = self.usbdev.take().expect("shouldn't happen");
        let devices = usbdev
            .comm
            .devices(proto::usbdev::RequestDevices {})?
            .devices;
        // Don't need you anymore
        usbdev.comm.end(proto::usbdev::RequestEnd {}).ok();
        Ok(devices)
    }

    fn select_device(&mut self) -> Result<()> {
        let devices = self.list_devices()?;
        if devices.is_empty() {
            log::error!("No device found");
            return Err(Error::Error("No device found".to_string()));
        }
        let index = if devices.len() == 1 {
            0
        } else {
            eprintln!("Multiple devices found, which one should be imaged ?");
            for (index, dev) in devices.iter().enumerate() {
                eprintln!(
                    "{}: {} - {} (Serial: {}, VID/PID: {}/{})",
                    index + 1,
                    dev.manufacturer,
                    dev.description,
                    dev.serial,
                    dev.vendorid,
                    dev.productid
                );
            }
            loop {
                eprint!("[1-{}]: ", devices.len());
                io::stdout().flush().expect("couldn't flush stdout");
                let mut input = String::new();
                match io::stdin().read_line(&mut input) {
                    Ok(_) => {
                        match input.trim().parse::<usize>() {
                            Ok(n) => {
                                if n > 0 && n <= devices.len() {
                                    break n - 1;
                                } else {
                                    log::error!("Index out of range");
                                }
                            }
                            Err(err) => {
                                log::error!("Couldn't parse input: {}", err);
                            }
                        };
                    }
                    Err(err) => {
                        log::error!("Couldn't read input: {}", err);
                    }
                }
            }
        };

        log::info!(
            "Cloning device \'{} - {} (Serial: {}, VID/PID: {}/{})\'",
            devices[index].manufacturer,
            devices[index].description,
            devices[index].serial,
            devices[index].vendorid,
            devices[index].productid
        );

        self.busdevnum = Some(BusDevNum {
            busnum: devices[index].busnum,
            devnum: devices[index].devnum,
        });
        Ok(())
    }

    fn image_device(&mut self) -> Result<()> {
        let BusDevNum { busnum, devnum } = self.busdevnum.take().expect("shouldn't happen");
        // Unlock dev2scsi
        let buf = (u64::from(devnum)) << 32 | u64::from(busnum);
        self.dev2scsi.comm.write_all(&buf.to_le_bytes())?;
        self.dev2scsi.locked = false;

        let rep: proto::scsi::Response = self.dev2scsi.comm.recv()?;
        let (dev_size, block_size) =
            if let Some(proto::scsi::response::Msg::OpenDevice(rep)) = rep.msg {
                (rep.dev_size, rep.block_size)
            } else {
                return Err(Error::Error("Couldn't open device".to_string()));
            };

        let mut todo = dev_size;
        let mut sector_count: u64 = READ_FILE_MAX_SIZE / block_size;
        let mut offset: u64 = 0;

        // Shiny progress bar
        let pb = indicatif::ProgressBar::new(dev_size);
        pb.set_style(
            indicatif::ProgressStyle::default_bar()
                .template("[{wide_bar}] {bytes}/{total_bytes} ({eta})")
                .map_err(|err| Error::Error(format!("progress bar err: {}", err)))?
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
        log::debug!("End usbsas children");
        if let Some(mut usbdev) = self.usbdev.take() {
            usbdev
                .comm
                .end(proto::usbdev::RequestEnd {})
                .expect("Couldn't end usbdev");
        }
        if self.dev2scsi.locked {
            self.dev2scsi
                .comm
                .write_all(&(0_u64).to_ne_bytes())
                .expect("Couldn't unlock dev2scsi");
        }
        self.dev2scsi
            .comm
            .end(proto::scsi::RequestEnd {})
            .expect("Couldn't end dev2scsi");
    }
}

fn main() -> Result<()> {
    env_logger::builder().format_timestamp(None).init();
    let matches = Command::new("usbsas-imager")
        .about("Clone a usb device (Mass Storage) with usbsas")
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
            Arg::new("output")
                .short('o')
                .long("output")
                .value_name("FILE")
                .help("Path of the output file")
                .conflicts_with("stdout")
                .num_args(1),
        )
        .arg(
            Arg::new("busnum")
                .short('b')
                .long("busnum")
                .requires("devnum")
                .value_name("BUSNUM")
                .value_parser(clap::value_parser!(u32))
                .help("Bus number of the device to clone")
                .num_args(1),
        )
        .arg(
            Arg::new("devnum")
                .short('d')
                .long("devnum")
                .requires("busnum")
                .value_name("DEVNUM")
                .value_parser(clap::value_parser!(u32))
                .help("Device number of the device to clone")
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
                log::error!("Couldn't create file {}: {}", path, err);
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

    let busdevnum = match (
        matches.get_one::<u32>("busnum"),
        matches.get_one::<u32>("devnum"),
    ) {
        (Some(busnum), Some(devnum)) => Some(BusDevNum {
            busnum: busnum.to_owned(),
            devnum: devnum.to_owned(),
        }),
        (None, Some(_)) | (Some(_), None) => {
            return Err(Error::Error(
                "Must specify both busnum and devnum".to_string(),
            ));
        }
        (None, None) => None,
    };

    let mut imager = Imager::new(config_path, writer, busdevnum)?;

    if imager.busdevnum.is_none() {
        imager.select_device()?;
    }

    imager.image_device()?;

    eprintln!("Device cloned successfully");

    Ok(())
}
