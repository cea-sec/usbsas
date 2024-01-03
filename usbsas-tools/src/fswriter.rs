//! Write a filesystem on a USB mass storage device (like dd) with usbsas.

use bitvec::prelude::*;
use clap::{Arg, Command};
use std::{
    fs::File,
    io::{prelude::*, SeekFrom},
    os::unix::io::AsRawFd,
};
use thiserror::Error;
use usbsas_comm::{protorequest, Comm};
use usbsas_process::{UsbsasChild, UsbsasChildSpawner};
use usbsas_proto as proto;
use usbsas_utils::SECTOR_SIZE;

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Arg(String),
    #[error("{0}")]
    Write(String),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("process: {0}")]
    Process(#[from] usbsas_process::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("progress error: {0}")]
    Progress(#[from] indicatif::style::TemplateError),
}
type Result<T> = std::result::Result<T, Error>;

protorequest!(
    CommFs2dev,
    fs2dev,
    size = DevSize[RequestDevSize, ResponseDevSize],
    startcopy = StartCopy[RequestStartCopy, ResponseStartCopy],
    wipe = Wipe[RequestWipe, ResponseWipe],
    loadbitvec = LoadBitVec[RequestLoadBitVec, ResponseLoadBitVec],
    end = End[RequestEnd, ResponseEnd]
);

struct FsWriter {
    fs2dev: UsbsasChild<proto::fs2dev::Request>,
    fs: File,
}

impl FsWriter {
    fn new(fs_path: String, busnum: u32, devnum: u32) -> Result<Self> {
        let fs = File::open(&fs_path)?;
        let mut fs2dev = UsbsasChildSpawner::new("usbsas-fs2dev")
            .arg(&fs_path)
            .wait_on_startup()
            .spawn::<proto::fs2dev::Request>()?;

        usbsas_sandbox::fswriter::seccomp(
            fs.as_raw_fd(),
            fs2dev.comm.input_fd(),
            fs2dev.comm.output_fd(),
        )?;

        // unlock fs2dev with busnum / devnum
        fs2dev.unlock_with(&(((u64::from(devnum)) << 32) | (u64::from(busnum))).to_ne_bytes())?;

        log::info!(
            "Writing fs '{}' on device BUS {} DEV {}",
            &fs_path,
            busnum,
            devnum
        );

        Ok(Self { fs2dev, fs })
    }

    fn write_fs(&mut self) -> Result<()> {
        // check fs size doesn't exceed dev size
        let fs_size = self.fs.seek(SeekFrom::End(0))?;
        self.fs.rewind()?;
        if fs_size % SECTOR_SIZE != 0 {
            return Err(Error::Write(format!(
                "fs size ({fs_size}) % sector size ({SECTOR_SIZE}) != 0"
            )));
        }
        let dev_size = self
            .fs2dev
            .comm
            .size(proto::fs2dev::RequestDevSize {})?
            .size;
        if fs_size > dev_size {
            return Err(Error::Write(format!(
                "filesystem size ({fs_size}) > device size ({dev_size}), aborting"
            )));
        }

        // Send 'full' bitvec to write the whole filesystem
        let mut bitvec = BitVec::<u8, Lsb0>::new();
        bitvec.resize((fs_size / SECTOR_SIZE) as usize, false);
        bitvec.fill(true);
        let mut chunks = bitvec.chunks(10 * 1024 * 1024).peekable();

        while let Some(chunk) = chunks.next() {
            self.fs2dev
                .comm
                .loadbitvec(proto::fs2dev::RequestLoadBitVec {
                    chunk: chunk.to_bitvec().into_vec(),
                    last: chunks.peek().is_none(),
                })?;
        }

        // start copy with shiny progress bar
        use proto::fs2dev::response::Msg;
        self.fs2dev
            .comm
            .startcopy(proto::fs2dev::RequestStartCopy {})?;
        let pb = indicatif::ProgressBar::new(fs_size);
        pb.set_style(
            indicatif::ProgressStyle::default_bar()
                .template("[{wide_bar}] {bytes}/{total_bytes} ({eta})")?
                .progress_chars("#>-"),
        );

        loop {
            let rep: proto::fs2dev::Response = self.fs2dev.comm.recv()?;
            match rep.msg.ok_or(Error::BadRequest)? {
                Msg::CopyStatus(status) => {
                    pb.set_position(status.current_size);
                }
                Msg::CopyStatusDone(_) => {
                    pb.set_position(fs_size);
                    break;
                }
                Msg::Error(msg) => return Err(Error::Write(msg.err)),
                _ => return Err(Error::Write("bad resp from fs2dev".to_string())),
            }
        }

        log::info!("Filesystem written successfully");

        Ok(())
    }
}

impl Drop for FsWriter {
    fn drop(&mut self) {
        if self.fs2dev.locked {
            self.fs2dev
                .comm
                .write_all(&(0_u64).to_ne_bytes())
                .expect("couldn't unlock fs2dev");
        }
        self.fs2dev
            .comm
            .end(proto::fs2dev::RequestEnd {})
            .expect("couldn't end fs2dev");
    }
}

fn main() -> Result<()> {
    env_logger::builder().format_timestamp(None).init();
    let command = Command::new("usbsas-fswriter")
        .about("Write a filesystem (from file) on a USB device (Mass Storage) with usbsas")
        .version("1.0")
        .arg(
            Arg::new("filesystem")
                .value_name("FILE")
                .index(1)
                .help("Path of the input filesystem")
                .num_args(1)
                .required(true),
        )
        .arg(
            Arg::new("busnum")
                .value_name("BUSNUM")
                .requires("devnum")
                .value_parser(clap::value_parser!(u32))
                .help("Bus number of the output device")
                .index(2)
                .num_args(1)
                .required(true),
        )
        .arg(
            Arg::new("devnum")
                .value_name("DEVNUM")
                .requires("busnum")
                .value_parser(clap::value_parser!(u32))
                .help("Device number of the output device")
                .index(3)
                .num_args(1)
                .required(true),
        );

    let matches = command.get_matches();

    let (fs_path, busnum, devnum) = match (
        matches.get_one::<String>("filesystem"),
        matches.get_one::<u32>("busnum"),
        matches.get_one::<u32>("devnum"),
    ) {
        (Some(fs), Some(bn), Some(dn)) => (fs, bn, dn),
        _ => return Err(Error::Arg("missing arg".to_string())),
    };

    let mut fswriter = FsWriter::new(fs_path.to_owned(), busnum.to_owned(), devnum.to_owned())?;
    fswriter.write_fs()?;

    Ok(())
}
