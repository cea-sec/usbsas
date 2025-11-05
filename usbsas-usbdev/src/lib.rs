//! usbdev is a process from usbsas responsible for detecting plugged /
//! unplugged USB devices.
//!
//! It uses udev monitoring system.

use log::{debug, error, info, trace};
use mio::{Events, Interest, Poll, Token};
use std::{
    collections::HashMap,
    ffi::OsStr,
    os::unix::io::AsRawFd,
    sync::{Arc, Mutex},
    thread,
};
use thiserror::Error;
use usbsas_comm::{ComRpUsbDev, ProtoRespCommon, ProtoRespUsbDev, ToFd};
use usbsas_config::{conf_parse, conf_read, UsbPortAccesses};
use usbsas_proto as proto;
use usbsas_proto::{common::UsbDevice, usbdev::request::Msg};

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[cfg(feature = "mock")]
    #[error("mock: {0}")]
    Mock(#[from] usbsas_mock::usbdev::Error),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("Poison error")]
    Poison,
    #[error("ParseInt error {0}")]
    ParseInt(#[from] std::num::ParseIntError),
    #[error("None value")]
    NoneValue,
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
impl<T> From<std::sync::PoisonError<T>> for Error {
    fn from(_: std::sync::PoisonError<T>) -> Error {
        Error::Poison
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// Thread for getting plugged devices at startup and handling udev events
fn handle_udev_events(
    current_devices: Arc<Mutex<CurrentDevices>>,
    config_path: &str,
) -> Result<()> {
    let config = conf_read(config_path)?;

    let monitor = udev::MonitorBuilder::new()?.match_subsystem_devtype("usb", "usb_device")?;
    let mut poll = Poll::new()?;

    usbsas_sandbox::usbdev::seccomp_thread(monitor.as_raw_fd(), poll.as_raw_fd())?;

    let config = conf_parse(&config)?;

    let mut socket = monitor.listen()?;
    let mut events = Events::with_capacity(1024);

    poll.registry().register(
        &mut socket,
        Token(0),
        Interest::READABLE | Interest::WRITABLE,
    )?;

    // Scan devices once and add the already plugged ones in our list
    let mut enumerator = udev::Enumerator::new()?;
    enumerator.match_subsystem("usb")?;

    let mut cur_dev = current_devices.lock()?;
    cur_dev.usb_port_accesses = config.usb_port_accesses;
    if let Some(usb_ports) = &cur_dev.usb_port_accesses {
        log::debug!("usb_port_accesses: {usb_ports:?}");
    };

    for dev in enumerator.scan_devices()? {
        // Only add mass storage devices
        if let Some(value) = dev.property_value("ID_USB_INTERFACES") {
            if value.to_string_lossy().contains(":080650:")
                || value.to_string_lossy().contains(":080250:")
            {
                if let Err(err) = cur_dev.add_device(&dev) {
                    log::error!("Couldn't add dev {dev:?} ({err})");
                }
            }
        }
    }
    drop(cur_dev);

    // Handle udev events
    loop {
        poll.poll(&mut events, None)?;

        for event in &events {
            if event.token() == Token(0) && event.is_writable() {
                for ev in socket.iter() {
                    match ev.event_type() {
                        udev::EventType::Add => {
                            if let Some(value) = ev.property_value("ID_USB_INTERFACES") {
                                if value.to_string_lossy().contains(":080650:")
                                    || value.to_string_lossy().contains(":080250:")
                                {
                                    if let Err(err) =
                                        current_devices.lock()?.add_device(&ev.device())
                                    {
                                        log::error!(
                                            "Couldn't add dev {:#?} ({})",
                                            ev.device(),
                                            err
                                        );
                                    }
                                }
                            }
                        }
                        udev::EventType::Remove => {
                            if let Err(err) = current_devices.lock()?.rm_device(&ev.device()) {
                                log::error!("Couldn't rm dev {:?} ({})", ev.device(), err);
                            }
                        }
                        _ => (),
                    }
                }
            }
        }
    }
}

pub struct CurrentDevices {
    devices: HashMap<String, UsbDevice>,
    usb_port_accesses: Option<UsbPortAccesses>,
}

impl CurrentDevices {
    fn new(usb_port_accesses: Option<UsbPortAccesses>) -> Self {
        CurrentDevices {
            devices: HashMap::new(),
            usb_port_accesses,
        }
    }

    fn add_device(&mut self, device: &udev::Device) -> Result<()> {
        let busnum = device
            .attribute_value("busnum")
            .ok_or(Error::NoneValue)?
            .to_string_lossy()
            .parse::<u8>()?;

        let mut dev_path = Vec::new();
        dev_path.push(busnum);
        for port in device
            .attribute_value("devpath")
            .ok_or(Error::NoneValue)?
            .to_string_lossy()
            .split('.')
        {
            dev_path.push(port.parse::<u8>()?)
        }

        // Check if device is connected to an allowed port if policy configured
        let (is_src, mut is_dst) = if let Some(ports) = &self.usb_port_accesses {
            (
                ports.ports_src.contains(&dev_path),
                ports.ports_dst.contains(&dev_path),
            )
        } else {
            (true, true)
        };

        // Never destination if optical disk reader
        if device
            .property_value("ID_USB_INTERFACES")
            .ok_or(Error::NoneValue)?
            .to_string_lossy()
            .contains(":080250:")
        {
            is_dst = false;
        }

        if !is_src && !is_dst {
            log::warn!(
                "Ignoring device plugged in unauthorized port {:?}",
                dev_path.as_slice()
            );
            return Ok(());
        }

        let dev = UsbDevice {
            busnum: busnum.into(),
            devnum: device
                .attribute_value("devnum")
                .ok_or(Error::NoneValue)?
                .to_string_lossy()
                .parse::<u32>()?,
            vendorid: u32::from_str_radix(
                &device
                    .attribute_value("idVendor")
                    .unwrap_or(OsStr::new("0"))
                    .to_string_lossy(),
                16,
            )?,
            productid: u32::from_str_radix(
                &device
                    .attribute_value("idProduct")
                    .unwrap_or(OsStr::new("0"))
                    .to_string_lossy(),
                16,
            )?,
            manufacturer: device
                .attribute_value("manufacturer")
                .unwrap_or(OsStr::new("unknown"))
                .to_string_lossy()
                .to_string(),
            description: device
                .attribute_value("product")
                .unwrap_or(OsStr::new("unknown"))
                .to_string_lossy()
                .to_string(),
            serial: device
                .attribute_value("serial")
                .unwrap_or(OsStr::new("unknown"))
                .to_string_lossy()
                .to_string(),
            dev_size: None,
            block_size: None,
            is_src,
            is_dst,
        };

        info!(
            "Device plugged {}-{} ({} - {} - {}) {:?}, src: {}, dst: {}]",
            dev.busnum,
            dev.devnum,
            dev.manufacturer,
            dev.description,
            dev.serial,
            dev_path.as_slice(),
            dev.is_src,
            dev.is_dst
        );

        self.devices
            .insert(device.syspath().to_string_lossy().to_string(), dev);

        Ok(())
    }

    fn rm_device(&mut self, device: &udev::Device) -> Result<()> {
        if let Some(dev) = self
            .devices
            .remove(&device.syspath().to_string_lossy().to_string())
        {
            debug!("see you {}-{}", dev.busnum, dev.devnum);
        }
        Ok(())
    }
}

enum State {
    Init(InitState),
    Running(RunningState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpUsbDev) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::Running(s) => s.run(comm),
            State::WaitEnd(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {
    config_path: String,
}

struct RunningState {
    current_devices: Arc<Mutex<CurrentDevices>>,
}

struct WaitEndState {}

impl InitState {
    fn run(self, comm: &mut ComRpUsbDev) -> Result<State> {
        trace!("init state");

        usbsas_sandbox::landlock(
            Some(&[
                &self.config_path,
                "/sys/bus",
                "/sys/class",
                "/sys/devices",
                "/run/udev",
            ]),
            None,
            None,
            None,
            None,
        )?;

        let current_devices = Arc::new(Mutex::new(CurrentDevices::new(None)));
        let cur_dev_clone = current_devices.clone();

        thread::spawn(move || handle_udev_events(cur_dev_clone, &self.config_path));

        usbsas_sandbox::usbdev::seccomp(comm.input_fd(), comm.output_fd())?;

        Ok(State::Running(RunningState { current_devices }))
    }
}

impl RunningState {
    fn run(self, comm: &mut ComRpUsbDev) -> Result<State> {
        trace!("running state");
        loop {
            let res = match comm.recv_req()? {
                Msg::Devices(_) => {
                    let mut devices = Vec::new();
                    self.current_devices
                        .lock()?
                        .devices
                        .values()
                        .for_each(|dev| devices.push(dev.clone()));
                    comm.devices(proto::usbdev::ResponseDevices { devices })
                }
                Msg::End(_) => {
                    comm.end()?;
                    break;
                }
            };
            match res {
                Ok(_) => continue,
                Err(err) => {
                    error!("{err}");
                    comm.error(err)?;
                }
            }
        }
        Ok(State::End)
    }
}

impl WaitEndState {
    fn run(self, comm: &mut ComRpUsbDev) -> Result<State> {
        trace!("wait end state");
        loop {
            match comm.recv_req()? {
                Msg::End(_) => {
                    comm.end()?;
                    break;
                }
                _ => {
                    error!("bad request");
                    comm.error("bad request")?;
                }
            }
        }
        Ok(State::End)
    }
}

pub struct UsbDev {
    comm: ComRpUsbDev,
    state: State,
}

impl UsbDev {
    pub fn new(comm: ComRpUsbDev, config_path: String) -> Result<Self> {
        Ok(UsbDev {
            comm,
            state: State::Init(InitState { config_path }),
        })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {err}, waiting end");
                    comm.error(err)?;
                    State::WaitEnd(WaitEndState {})
                }
            }
        }
        Ok(())
    }
}
