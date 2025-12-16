#![cfg(feature = "integration-tests")]

use assert_cmd::prelude::*;
use hex_literal::hex;
use nix::{
    sys::signal::{self, SIGTERM},
    unistd::Pid,
};
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use std::{
    env,
    error::Error,
    fs,
    io::{self, Write},
    process::{Child, Command},
    thread::sleep,
    time::Duration,
};
use usbsas_comm::{ComRqUsbsas, ProtoReqCommon, ProtoReqUsbsas};
use usbsas_process::{UsbsasChild, UsbsasChildSpawner};
use usbsas_proto::{
    self as proto,
    common::{device::Device, FsType, Status},
};

const OUT_DEV_SIZE: usize = 10;

struct IntegrationTester {
    wdir: String,
    conf_file: String,
    av_dir: String,
    client: reqwest::blocking::Client,
    usbsas: UsbsasChild<ComRqUsbsas>,
    analyzer_server: Child,
}

impl IntegrationTester {
    fn new() -> Self {
        let test_data_dir =
            env::var("CARGO_MANIFEST_DIR").expect("missing env var") + "/tests/resources";
        let conf_file = format!("{test_data_dir}/config_test.toml");

        // create dirs
        let wdir = String::from("/tmp/usbsas-tests");
        let av_dir = format!("{wdir}/av");
        for dir in &[&wdir, &av_dir] {
            if let Err(err) = fs::create_dir(dir) {
                if err.kind() != io::ErrorKind::AlreadyExists {
                    panic!("couldn't create \"{dir}\": {err}")
                }
            }
        }

        env::set_var("USBSAS_SESSION_ID", "integration-tests");

        // Start analyzer server
        let analyzer_server = Command::cargo_bin("usbsas-analyzer-server")
            .expect("Couldn't run analyzer server")
            .args(["-d", &av_dir])
            //.stdout(Stdio::null())
            .spawn()
            .expect("Couldn't run analyzer server");

        // uncompress mock input dev in working dir
        let mock_in_dev = format!("{wdir}/mock_in_dev.img");
        let mut mock_file = fs::File::create(&mock_in_dev).expect("couldn't create file");

        let file_reader = io::BufReader::new(
            fs::File::open(format!("{}/{}", test_data_dir, "mock_in_dev.img.gz")).unwrap(),
        );
        let mut decoder = flate2::bufread::GzDecoder::new(file_reader);
        io::copy(&mut decoder, &mut mock_file).expect("decode input file");
        env::set_var("USBSAS_MOCK_IN_DEV", mock_in_dev);

        // create mock output dev
        let mock_out_dev = format!("{wdir}/mock_out_dev.img");
        let zero_buf = vec![0; 1024 * 1024];
        let mut out_file = fs::File::create(&mock_out_dev).expect("out file");
        for _ in 0..OUT_DEV_SIZE {
            out_file.write_all(&zero_buf).expect("zeroing out file");
        }
        env::set_var("USBSAS_MOCK_OUT_DEV", mock_out_dev);

        // Copy export bundle in working dir
        let _ = fs::create_dir(format!("{av_dir}/Tartempion"));
        fs::copy(
            format!("{test_data_dir}/bundle_test.tar.gz"),
            format!("{av_dir}/Tartempion/123456.tar.gz"),
        )
        .expect("copy file");

        // Start usbsas
        //env::set_var("RUST_LOG", "error");
        let usbsas = UsbsasChildSpawner::new("usbsas-usbsas")
            .args(&["-c", &conf_file])
            .spawn::<ComRqUsbsas>()
            .expect("can't spawn usbsas");

        // web client to talk to analyzer-server
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("couldn't build reqwest client");

        // Wait for analyzer server to be ready
        loop {
            match client.get("http://127.0.0.1:8042/").send() {
                Ok(_) => break,
                Err(_) => {
                    sleep(Duration::from_secs(2));
                }
            }
        }

        IntegrationTester {
            wdir,
            conf_file,
            av_dir,
            client,
            usbsas,
            analyzer_server,
        }
    }

    fn comm(&mut self) -> &mut ComRqUsbsas {
        &mut self.usbsas.comm
    }

    fn reset(&mut self) {
        // end usbsas
        self.usbsas.comm.end().expect("end usbsas");

        // reset out file
        let mock_out_dev = format!("{}/mock_out_dev.img", self.wdir);
        let zero_buf = vec![0; 1024 * 1024];
        let mut out_file = fs::File::create(&mock_out_dev).expect("out file");
        for _ in 0..OUT_DEV_SIZE {
            out_file.write_all(&zero_buf).expect("zeroing out file");
        }

        // start new usbsas
        self.usbsas = UsbsasChildSpawner::new("usbsas-usbsas")
            .args(&["-c", &self.conf_file])
            .spawn::<ComRqUsbsas>()
            .expect("can't spawn usbsas");
    }

    // USB to USB transfer
    // read FAT partition, write NTFS partition
    fn usb_to_usb(&mut self) -> Result<(), Box<dyn Error>> {
        let devices = self
            .comm()
            .devices(proto::usbsas::RequestDevices { include_alt: false })?
            .devices;
        assert_eq!(devices.len(), 2);
        let in_dev = devices
            .iter()
            .find(|dev| {
                if let Some(device) = &(dev).device {
                    matches!(device, Device::Usb(_)) && device.is_src()
                } else {
                    false
                }
            })
            .expect("couldn't find input device");
        let out_dev = devices
            .iter()
            .find(|dev| {
                if let Some(device) = &(dev).device {
                    matches!(device, Device::Usb(_)) && device.is_dst()
                } else {
                    false
                }
            })
            .expect("couldn't find out device");

        let id = self.comm().userid(proto::usbsas::RequestUserId {})?.userid;
        assert_eq!(id, "Tartempion");

        self.comm()
            .inittransfer(proto::usbsas::RequestInitTransfer {
                source: in_dev.id,
                destination: out_dev.id,
                fstype: Some(FsType::Ntfs.into()),
                pin: None,
            })?;
        let parts = self
            .comm()
            .partitions(proto::usbsas::RequestPartitions {})?
            .partitions;
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1].type_str, "FAT");
        let _ = self
            .comm()
            .openpartition(proto::usbsas::RequestOpenPartition { index: 1 })?;

        let files = self
            .comm()
            .readdir(proto::usbsas::RequestReadDir { path: "/".into() })?
            .filesinfo;
        assert_eq!(files.len(), 7);

        let _ = self.comm().selectfiles(proto::usbsas::RequestSelectFiles {
            selected: vec!["/".into()],
        })?;

        loop {
            let resp = self.usbsas.comm.recv_status()?;
            if resp.status == Status::AllDone.into() {
                break;
            }
        }

        let report = self
            .comm()
            .report(proto::usbsas::RequestReport {})?
            .report
            .unwrap();

        assert_eq!(
            report.file_names,
            [
                "/chicken.pdf",
                "/quiche/lorem ipsum.txt",
                "/quiche/plop/random.bin",
                "/tree 🌲.txt",
                "/usbsas-logo.svg"
            ]
        );
        assert_eq!(
            report.filtered,
            ["/AUTORUN.INF", "/Micro$oft.lnk", "/quiche/.DS_STORE"]
        );
        assert_eq!(report.rejected, ["infected/eicar.com"]);

        assert_eq!(
            sha256(&env::var("USBSAS_MOCK_OUT_DEV")?)?,
            hex!("fdef28f25854edf7ccefffda706243295d696455c73fee406fa325912e90b44c")
        );
        Ok(())
    }

    // USB to network transfer
    // read NTFS partition
    fn usb_to_net(&mut self) -> Result<(), Box<dyn Error>> {
        let devices = self
            .comm()
            .devices(proto::usbsas::RequestDevices { include_alt: true })?
            .devices;
        let in_dev = devices
            .iter()
            .find(|dev| {
                if let Some(device) = &(dev).device {
                    matches!(device, Device::Usb(_)) && device.is_src()
                } else {
                    false
                }
            })
            .expect("couldn't find input device");
        let out_dev = devices
            .iter()
            .find(|dev| {
                if let Some(device) = &(dev).device {
                    matches!(device, Device::Network(_)) && device.is_dst()
                } else {
                    false
                }
            })
            .expect("couldn't find out device");

        let id = self.comm().userid(proto::usbsas::RequestUserId {})?.userid;
        assert_eq!(id, "Tartempion");

        self.comm()
            .inittransfer(proto::usbsas::RequestInitTransfer {
                source: in_dev.id,
                destination: out_dev.id,
                fstype: None,
                pin: None,
            })?;
        let parts = self
            .comm()
            .partitions(proto::usbsas::RequestPartitions {})?
            .partitions;
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].type_str, "NTFS");
        let _ = self
            .comm()
            .openpartition(proto::usbsas::RequestOpenPartition { index: 0 })?;

        let _ = self.comm().selectfiles(proto::usbsas::RequestSelectFiles {
            selected: vec!["/".into()],
        })?;

        loop {
            let resp = self.usbsas.comm.recv_status()?;
            if resp.status == Status::AllDone.into() {
                break;
            }
        }

        assert_eq!(
            sha256(&format!("{}/bundle_test.tar", self.av_dir))?,
            hex!("e47b5ae1ce68b002f52c31322b21de629d0a24bc139205208b10e903da6ddb9a")
        );

        Ok(())
    }

    // Network to USB
    // write exfat partition
    fn net_to_usb(&mut self) -> Result<(), Box<dyn Error>> {
        let devices = self
            .comm()
            .devices(proto::usbsas::RequestDevices { include_alt: true })?
            .devices;
        let in_dev = devices
            .iter()
            .find(|dev| {
                if let Some(device) = &(dev).device {
                    matches!(device, Device::Network(_)) && device.is_src()
                } else {
                    false
                }
            })
            .expect("couldn't find input device");
        let out_dev = devices
            .iter()
            .find(|dev| {
                if let Some(device) = &(dev).device {
                    matches!(device, Device::Usb(_)) && device.is_dst()
                } else {
                    false
                }
            })
            .expect("couldn't find out device");

        let _ = self.comm().userid(proto::usbsas::RequestUserId {})?.userid;

        self.comm()
            .inittransfer(proto::usbsas::RequestInitTransfer {
                source: in_dev.id,
                destination: out_dev.id,
                fstype: Some(FsType::Exfat.into()),
                pin: Some("123456".into()),
            })?;

        loop {
            let resp = self.usbsas.comm.recv_status()?;
            if resp.status == Status::AllDone.into() {
                break;
            }
        }

        assert_eq!(
            sha256(&env::var("USBSAS_MOCK_OUT_DEV")?)?,
            hex!("236ce8ea48edd1f7ed3fe0d9a9625e1267870bba47e7d38542623697c220eeb4")
        );

        Ok(())
    }

    // USB to cmd transfer
    // read ext4 partition
    fn usb_to_cmd(&mut self) -> Result<(), Box<dyn Error>> {
        let devices = self
            .comm()
            .devices(proto::usbsas::RequestDevices { include_alt: true })?
            .devices;
        let in_dev = devices
            .iter()
            .find(|dev| {
                if let Some(device) = &(dev).device {
                    matches!(device, Device::Usb(_)) && device.is_src()
                } else {
                    false
                }
            })
            .expect("couldn't find input device");
        let out_dev = devices
            .iter()
            .find(|dev| {
                if let Some(device) = &(dev).device {
                    matches!(device, Device::Command(_)) && device.is_dst()
                } else {
                    false
                }
            })
            .expect("couldn't find out device");

        let _ = self.comm().userid(proto::usbsas::RequestUserId {})?.userid;

        self.comm()
            .inittransfer(proto::usbsas::RequestInitTransfer {
                source: in_dev.id,
                destination: out_dev.id,
                fstype: Some(FsType::Ntfs.into()),
                pin: None,
            })?;
        let parts = self
            .comm()
            .partitions(proto::usbsas::RequestPartitions {})?
            .partitions;
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[2].type_str, "Linux/Ext");
        let _ = self
            .comm()
            .openpartition(proto::usbsas::RequestOpenPartition { index: 2 })?;

        let _ = self.comm().selectfiles(proto::usbsas::RequestSelectFiles {
            selected: vec!["/".into()],
        })?;

        loop {
            let resp = self.usbsas.comm.recv_status()?;
            if resp.status == Status::AllDone.into() {
                break;
            }
        }

        assert_eq!(
            sha256(&format!("{}/quiche.tar", self.wdir))?,
            hex!("f79d9af346cbb1389e1c7c5cff0e3ee5b3bf821b0b3c99ab7f6ebd2c0273b7bf")
        );

        Ok(())
    }

    fn wipe(&mut self) -> Result<(), Box<dyn Error>> {
        let devices = self
            .usbsas
            .comm
            .devices(proto::usbsas::RequestDevices { include_alt: false })?
            .devices;
        let out_dev = devices
            .iter()
            .find(|dev| {
                if let Some(device) = &(dev).device {
                    matches!(device, Device::Usb(_)) && device.is_dst()
                } else {
                    false
                }
            })
            .expect("couldn't find out device");

        self.usbsas.comm.wipe(proto::usbsas::RequestWipe {
            id: out_dev.id,
            fstype: FsType::Ntfs.into(),
            quick: true,
        })?;

        loop {
            let resp = self.usbsas.comm.recv_status()?;
            if resp.status == Status::AllDone.into() {
                break;
            }
        }

        assert_eq!(
            sha256(&env::var("USBSAS_MOCK_OUT_DEV")?)?,
            hex!("bf6cc60d79898f54bee2f9c2375c6992580d0f0053f3e6d78250345df12af238")
        );

        Ok(())
    }
}

impl Drop for IntegrationTester {
    fn drop(&mut self) {
        let _ = self.usbsas.comm.end();
        sleep(Duration::from_secs(1));
        // Send SIGTERM instead of SIGKILL so the signal is forwarded to sons
        while let Err(err) = signal::kill(Pid::from_raw(self.usbsas.child.id() as i32), SIGTERM) {
            println!("Couldn't sigterm usbsas server: {err}");
            sleep(Duration::from_secs(1));
        }
        let _ = self.client.get("http://localhost:8042/shutdown").send();
        sleep(Duration::from_secs(1));
        if let Err(e) = self.analyzer_server.kill() {
            println!("Couldn't kill analyzer server: {e}");
        }
        // Remove working dir
        let _ = fs::remove_dir_all(&self.wdir).ok();
    }
}

fn sha256(path: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let _ = io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().to_vec())
}

#[test]
fn integration_tests() {
    let mut tester = IntegrationTester::new();

    tester.usb_to_usb().expect("usb_to_usb test");

    tester.reset();
    tester.usb_to_net().expect("usb_to_net test");

    tester.reset();
    tester.net_to_usb().expect("net_to_usb test");

    tester.reset();
    tester.usb_to_cmd().expect("usb_to_cmd test");

    tester.reset();
    tester.wipe().expect("wipe test");
}
