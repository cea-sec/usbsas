//! usbdev is a process from usbsas responsible for detecting plugged /
//! unplugged USB devices.
//!
//! It uses libusb's hot-plug handler.

use log::{debug, error, info, trace};
use rusb::constants::LIBUSB_CLASS_MASS_STORAGE;
use rusb::UsbContext;
use std::{
    os::unix::io::RawFd,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use thiserror::Error;
use usbsas_comm::{protoresponse, Comm};
use usbsas_config::{conf_parse, conf_read, UsbPortAccesses};
use usbsas_process::UsbsasProcess;
use usbsas_proto as proto;
use usbsas_proto::{common::Device as UsbDevice, usbdev::request::Msg};

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("rusb error: {0}")]
    Rusb(#[from] rusb::Error),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("Poison error")]
    Poison,
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
type Result<T> = std::result::Result<T, Error>;

protoresponse!(
    CommUsbdev,
    usbdev,
    devices = Devices[ResponseDevices],
    error = Error[ResponseError],
    end = End[ResponseEnd]
);

struct HotPlugHandler {
    current_devices: Arc<Mutex<CurrentDevices>>,
}

impl<T: rusb::UsbContext> rusb::Hotplug<T> for HotPlugHandler {
    fn device_arrived(&mut self, device: rusb::Device<T>) {
        debug!("hello there {:#?}", device);
        match self.current_devices.lock() {
            Ok(mut cur_dev_guard) => {
                if let Err(err) = cur_dev_guard.add_dev(device) {
                    error!("Couldn't add dev: {}", err);
                }
            }
            Err(err) => {
                error!("Couldn't get lock {}", err);
            }
        }
    }

    fn device_left(&mut self, device: rusb::Device<T>) {
        debug!("see you {:#?}", device);
        match self.current_devices.lock() {
            Ok(mut cur_dev_guard) => cur_dev_guard.rm_dev(device),
            Err(err) => {
                error!("Couldn't get lock {}", err);
            }
        }
    }
}

/// Handle libusb hotplug events
fn handle_events_loop(
    context: rusb::Context,
    current_devices: Arc<Mutex<CurrentDevices>>,
) -> Result<()> {
    usbsas_sandbox::usbdev::thread_seccomp(usbsas_sandbox::get_libusb_opened_fds(0, 0)?)?;
    loop {
        trace!("waiting libusb event");
        if let Err(err) = context.handle_events(None) {
            error!("Couldn't handle libusb events: {}", err);
            continue;
        }
        trace!("handled libusb event");

        match current_devices.lock() {
            Ok(ref mut cur_dev_guard) => {
                if cur_dev_guard.need_update {
                    cur_dev_guard.update_desc_last(&context);
                }
            }
            Err(err) => {
                error!("Couldn't get devices lock {}", err);
                continue;
            }
        };
    }
}

/// Compare vectors (usb ports)
fn cmp_vec(vec1: &[u32], vec2: &[u8]) -> bool {
    if vec1.len() != vec2.len() {
        return false;
    }
    vec1.iter()
        .zip(vec2.iter())
        .all(|(&elt1, &elt2)| elt1 == elt2 as u32)
}

pub struct CurrentDevices {
    devices: Vec<UsbDevice>,
    need_update: bool,
    usb_port_accesses: Option<UsbPortAccesses>,
}

impl CurrentDevices {
    fn new(usb_port_accesses: Option<UsbPortAccesses>) -> Self {
        CurrentDevices {
            devices: Vec::new(),
            need_update: false,
            usb_port_accesses,
        }
    }

    /// Function called when a device is plugged. It will save its information
    fn add_dev<T: rusb::UsbContext>(&mut self, device: rusb::Device<T>) -> Result<bool> {
        let ports: Vec<u32> = match device.port_numbers() {
            Ok(ports) => ports.iter().map(|&val| val as u32).collect(),
            Err(err) => {
                error!("couldn't get device port numbers: {}", err);
                return Err(err.into());
            }
        };

        let (is_src, is_dst) = if let Some(usb_port_accesses) = &self.usb_port_accesses {
            if cmp_vec(&ports, &usb_port_accesses.ports_src) {
                (true, false)
            } else if cmp_vec(&ports, &usb_port_accesses.ports_dst) {
                (false, true)
            } else {
                debug!("Device plugged in unhandled port {:?}", ports);
                return Ok(false);
            }
        } else {
            (true, true)
        };

        let descriptor = match device.device_descriptor() {
            Ok(des) => des,
            Err(err) => {
                error!("couldn't get device descriptor: {}", err);
                return Err(err.into());
            }
        };

        for n in 0..descriptor.num_configurations() {
            let config_desc = match device.config_descriptor(n) {
                Ok(cfd) => cfd,
                Err(err) => {
                    error!("couldn't get config descriptor: {}", err);
                    return Err(err.into());
                }
            };
            for interface in config_desc.interfaces() {
                for interface_desc in interface.descriptors() {
                    if interface_desc.class_code() == LIBUSB_CLASS_MASS_STORAGE {
                        self.devices.push(UsbDevice {
                            busnum: device.bus_number() as u32,
                            devnum: device.address() as u32,
                            vendorid: descriptor.vendor_id() as u32,
                            productid: descriptor.product_id() as u32,
                            manufacturer: "unknown".into(),
                            description: "unknown".into(),
                            serial: "unknown".into(),
                            is_src,
                            is_dst,
                        });
                        self.need_update = true;
                        return Ok(true);
                    }
                }
            }
        }

        Ok(false)
    }

    /// Remove device from current list
    fn rm_dev<T: rusb::UsbContext>(&mut self, device: rusb::Device<T>) {
        if let Some(index) = self.devices.iter().position(|x| {
            (x.busnum, x.devnum) == (device.bus_number() as u32, device.address() as u32)
        }) {
            self.devices.remove(index);
        }
    }

    /// Get more information on last plugged device like its description and
    /// manufacturer etc.
    fn update_desc_last(&mut self, context: &rusb::Context) {
        let last_device = match self.devices.last_mut() {
            Some(dev) => dev,
            None => return,
        };

        let devices = match context.devices() {
            Ok(devs) => devs,
            Err(err) => {
                error!("Couldn't get devices: {}", err);
                return;
            }
        };

        for device in devices.iter() {
            if device.bus_number() == last_device.busnum as u8
                && device.address() == last_device.devnum as u8
            {
                match device.open() {
                    Ok(mut handle) => {
                        if let Err(err) = handle.reset() {
                            error!("Couldn't reset device: {}", err);
                            return;
                        };
                        let timeout = Duration::from_secs(1);
                        let languages = match handle.read_languages(timeout) {
                            Ok(lang) => lang,
                            Err(err) => {
                                error!("Couldn't read languages: {}", err);
                                return;
                            }
                        };

                        let device_descriptor = match handle.device().device_descriptor() {
                            Ok(dd) => dd,
                            Err(err) => {
                                error!("couldn't get device descriptor: {}", err);
                                return;
                            }
                        };
                        let (manufacturer, description, serial) = if !languages.is_empty() {
                            (
                                handle
                                    .read_manufacturer_string(
                                        languages[0],
                                        &device_descriptor,
                                        timeout,
                                    )
                                    .unwrap_or_else(|_| "Unknown manufacturer".to_string()),
                                handle
                                    .read_product_string(languages[0], &device_descriptor, timeout)
                                    .unwrap_or_else(|_| "Unknown description".to_string()),
                                handle
                                    .read_serial_number_string(
                                        languages[0],
                                        &device_descriptor,
                                        timeout,
                                    )
                                    .unwrap_or_else(|_| "Unknown serial".to_string()),
                            )
                        } else {
                            return;
                        };
                        info!(
                            "Device plugged: {} - {} - {}",
                            manufacturer, description, serial
                        );
                        last_device.manufacturer = manufacturer;
                        last_device.description = description;
                        last_device.serial = serial;
                        self.need_update = false;
                    }
                    Err(err) => {
                        error!("Can't open dev: {}", err);
                    }
                }
            }
        }
    }

    fn add_dev_with_desc<T: rusb::UsbContext>(
        &mut self,
        device: rusb::Device<T>,
        context: &rusb::Context,
    ) {
        match self.add_dev(device) {
            Ok(true) => self.update_desc_last(context),
            Ok(false) => (),
            Err(err) => {
                error!("Couldn't add dev: {}", err);
            }
        }
    }
}

enum State {
    Init(InitState),
    Running(RunningState),
    WaitEnd(WaitEndState),
    End,
}

impl State {
    fn run(self, comm: &mut Comm<proto::usbdev::Request>) -> Result<Self> {
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
    context: rusb::Context,
    current_devices: Arc<Mutex<CurrentDevices>>,
    registration: rusb::Registration<rusb::Context>,
}

struct WaitEndState {}

impl InitState {
    fn run(self, comm: &mut Comm<proto::usbdev::Request>) -> Result<State> {
        trace!("init state");
        let config = conf_parse(&conf_read(&self.config_path)?)?;

        let context = rusb::Context::new()?;
        let current_devices = Arc::new(Mutex::new(CurrentDevices::new(config.usb_port_accesses)));

        // Poll devices
        for device in context.devices()?.iter() {
            current_devices.lock()?.add_dev_with_desc(device, &context);
        }

        let registration = rusb::HotplugBuilder::new()
            // .class(LIBUSB_CLASS_MASS_STORAGE) // doesn't seem to work
            .register(
                &context,
                Box::new(HotPlugHandler {
                    current_devices: current_devices.clone(),
                }),
            )?;

        let context_clone = context.clone();
        let cur_dev_clone = current_devices.clone();
        thread::spawn(|| handle_events_loop(context_clone, cur_dev_clone));

        usbsas_sandbox::usbdev::seccomp(
            comm.input_fd(),
            comm.output_fd(),
            usbsas_sandbox::get_libusb_opened_fds(0, 0)?,
        )?;

        Ok(State::Running(RunningState {
            context,
            current_devices,
            registration,
        }))
    }
}

impl RunningState {
    fn run(self, comm: &mut Comm<proto::usbdev::Request>) -> Result<State> {
        trace!("running state");
        loop {
            let req: proto::usbdev::Request = comm.recv()?;
            let res = match req.msg.ok_or(Error::BadRequest)? {
                Msg::Devices(_) => comm.devices(proto::usbdev::ResponseDevices {
                    devices: self.current_devices.lock()?.devices.clone(),
                }),
                Msg::End(_) => {
                    self.context.unregister_callback(self.registration);
                    comm.end(proto::usbdev::ResponseEnd {})?;
                    break;
                }
            };
            match res {
                Ok(_) => continue,
                Err(err) => {
                    error!("{}", err);
                    comm.error(proto::usbdev::ResponseError {
                        err: format!("{}", err),
                    })?;
                }
            }
        }
        Ok(State::End)
    }
}

impl WaitEndState {
    fn run(self, comm: &mut Comm<proto::usbdev::Request>) -> Result<State> {
        trace!("wait end state");
        loop {
            let req: proto::usbdev::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::End(_) => {
                    comm.end(proto::usbdev::ResponseEnd {})?;
                    break;
                }
                _ => {
                    error!("bad request");
                    comm.error(proto::usbdev::ResponseError {
                        err: "bad req, waiting end".into(),
                    })?;
                }
            }
        }
        Ok(State::End)
    }
}

pub struct UsbDev {
    comm: Comm<proto::usbdev::Request>,
    state: State,
}

impl UsbDev {
    fn new(comm: Comm<proto::usbdev::Request>, config_path: String) -> Result<Self> {
        if !rusb::has_hotplug() {
            error!("libusb doesn't support hotplug");
            std::process::exit(1);
        }
        Ok(UsbDev {
            comm,
            state: State::Init(InitState { config_path }),
        })
    }

    fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm) {
                Ok(State::End) => break,
                Ok(state) => state,
                Err(err) => {
                    error!("state run error: {}, waiting end", err);
                    comm.error(proto::usbdev::ResponseError {
                        err: format!("run error: {}", err),
                    })?;
                    State::WaitEnd(WaitEndState {})
                }
            }
        }
        Ok(())
    }
}

impl UsbsasProcess for UsbDev {
    fn spawn(
        read_fd: RawFd,
        write_fd: RawFd,
        args: Option<Vec<String>>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        if let Some(args) = args {
            if args.len() == 1 {
                UsbDev::new(Comm::from_raw_fd(read_fd, write_fd), args[0].to_owned())?
                    .main_loop()
                    .map(|_| debug!("usbdev: exiting"))?;
                return Ok(());
            }
        }
        Err(Box::new(Error::Error(
            "usbdev needs a config_path arg".to_string(),
        )))
    }
}
