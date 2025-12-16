use log::trace;
use std::env;
use thiserror::Error;
use usbsas_comm::{ComRpUsbDev, ProtoRespCommon, ProtoRespUsbDev};
use usbsas_proto as proto;
use usbsas_proto::{common::UsbDevice, usbdev::request::Msg};

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("Bad Request")]
    BadRequest,
}
pub type Result<T> = std::result::Result<T, Error>;

pub struct MockUsbDev {
    comm: ComRpUsbDev,
    devices: Vec<UsbDevice>,
}

impl MockUsbDev {
    pub fn new(comm: ComRpUsbDev, _: String) -> Result<Self> {
        let mut devices = Vec::new();

        // Mock input device
        if env::var("USBSAS_MOCK_IN_DEV").is_ok() {
            devices.push(UsbDevice {
                busnum: 1,
                devnum: 1, // 1 = INPUT
                vendorid: 1,
                productid: 1,
                manufacturer: "manufacturer".to_string(),
                description: "mock input dev".to_string(),
                serial: "serial".to_string(),
                is_src: true,
                is_dst: false,
                block_size: None,
                dev_size: None,
            });
        }

        // Mock output device
        if env::var("USBSAS_MOCK_OUT_DEV").is_ok() {
            devices.push(UsbDevice {
                busnum: 1,
                devnum: 2, // 2 = OUTPUT
                vendorid: 1,
                productid: 1,
                manufacturer: "manufacturer".to_string(),
                description: "mock output dev".to_string(),
                serial: "serial".to_string(),
                is_src: false,
                is_dst: true,
                block_size: None,
                dev_size: None,
            });
        }

        Ok(MockUsbDev { comm, devices })
    }

    fn handle_req_devices(&mut self) -> Result<()> {
        self.comm
            .devices(proto::usbdev::ResponseDevices {
                devices: self.devices.clone(),
            })
            .map_err(|e| e.into())
    }

    pub fn main_loop(&mut self) -> Result<()> {
        trace!("main loop");
        loop {
            let res = match self.comm.recv_req()? {
                Msg::Devices(_) => self.handle_req_devices(),
                Msg::End(_) => {
                    self.comm.end()?;
                    break;
                }
            };
            match res {
                Ok(_) => continue,
                Err(err) => {
                    self.comm.error(err)?;
                }
            }
        }
        Ok(())
    }
}
