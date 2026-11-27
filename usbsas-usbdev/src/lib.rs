//! usbdev is a process from usbsas responsible for detecting plugged /
//! unplugged USB devices.
//!
//! It uses udev monitoring system. Three threads, each with its own Landlock
//! domain and seccomp filter:
//! - the main thread answers the parent, no file system access;
//! - the monitor thread receives udev events and builds devices from their
//!   properties only, no file system access;
//! - the scan thread, started once the monitor listens, lists the devices
//!   already plugged (the only one reading sysfs and the udev database), sends
//!   them to the main thread and exits.
//!
//! Only mass storage devices (SCSI bulk only, or optical disk readers which are
//! never destinations) are listed. With `usb_port_accesses` configured, devices
//! are sources or destinations depending on the port they're plugged in.
//!
//! States: `Init` (sandbox, start the udev thread) -> `Running` (`Devices`) ->
//! `End`.

use log::{error, info, trace};
use mio::{Events, Interest, Poll, Token};
use std::{
    collections::HashMap,
    os::unix::io::AsRawFd,
    sync::{Arc, Mutex, mpsc},
    thread,
};
use thiserror::Error;
use usbsas_comm::{ComRpUsbDev, ProtoRespCommon, ProtoRespUsbDev, ToFd};
use usbsas_config::{UsbPortAccesses, conf_parse, conf_read};
use usbsas_proto as proto;
use usbsas_proto::{common::UsbDevice, usbdev::request::Msg};

/// Errors of this process.
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
/// Result type of this process.
pub type Result<T> = std::result::Result<T, Error>;

/// Mass storage devices: SCSI bulk only (`:080650:`) or optical disk readers
/// (`:080250:`), as listed in the `ID_USB_INTERFACES` property.
fn is_mass_storage(device: &udev::Device) -> bool {
    device
        .property_value("ID_USB_INTERFACES")
        .map(|value| value.to_string_lossy())
        .is_some_and(|value| value.contains(":080650:") || value.contains(":080250:"))
}

/// Properties of a device, by name.
type Props = HashMap<String, String>;

/// Devices found by the scan thread: sysfs path and properties.
type Scanned = Vec<(String, Props)>;

/// List the mass storage devices already plugged: their sysfs path and
/// properties (read from sysfs and the udev database).
fn scan_devices() -> Result<Scanned> {
    let mut enumerator = udev::Enumerator::new()?;
    enumerator.match_subsystem("usb")?;
    Ok(enumerator
        .scan_devices()?
        .filter(is_mass_storage)
        .map(|dev| {
            let props = dev
                .properties()
                .map(|prop| {
                    (
                        prop.name().to_string_lossy().into_owned(),
                        prop.value().to_string_lossy().into_owned(),
                    )
                })
                .collect();
            (dev.syspath().to_string_lossy().into_owned(), props)
        })
        .collect())
}

/// Scan thread: sandbox, scan the devices already plugged, send them to the
/// main thread and exit. It inherits the read access to sysfs and the udev
/// database of the main thread's Landlock domain. On error, nothing is sent.
fn scan_thread(scanned: mpsc::SyncSender<Scanned>) -> Result<()> {
    usbsas_sandbox::usbdev::seccomp_scan()?;
    scanned
        .send(scan_devices()?)
        .map_err(|_| Error::Error("main thread exited".into()))
}

/// Monitor thread: set up the monitor, drop the file system access inherited
/// from the main thread, sandbox, then tell the main thread it listens (so the
/// scan can start without missing a device plugged in between). Events are
/// only handled once the main thread has added the scanned devices with the
/// `usb_port_accesses` policy (until then they wait in the monitor socket).
fn handle_udev_events(
    current_devices: Arc<Mutex<CurrentDevices>>,
    listening: mpsc::SyncSender<()>,
    start: mpsc::Receiver<()>,
) -> Result<()> {
    // Set up the monitor (bind, socket filter) and the poll instance before
    // seccomp.
    let monitor = udev::MonitorBuilder::new()?.match_subsystem_devtype("usb", "usb_device")?;
    let mut poll = Poll::new()?;
    let mut socket = monitor.listen()?;
    let mut events = Events::with_capacity(1024);

    poll.registry().register(
        &mut socket,
        Token(0),
        Interest::READABLE | Interest::WRITABLE,
    )?;

    usbsas_sandbox::landlock(None, None, None, None, None)?;
    usbsas_sandbox::usbdev::seccomp_thread(socket.as_raw_fd(), poll.as_raw_fd())?;

    listening
        .send(())
        .map_err(|_| Error::Error("main thread exited".into()))?;
    start
        .recv()
        .map_err(|_| Error::Error("start not received".into()))?;

    // Handle udev events. Only the properties sent with the event are read
    // (never sysfs, nor `Device`'s `Debug` which reads the parent device).
    loop {
        poll.poll(&mut events, None)?;

        for event in &events {
            if event.token() == Token(0) && event.is_writable() {
                for ev in socket.iter() {
                    match ev.event_type() {
                        udev::EventType::Add => {
                            if is_mass_storage(&ev)
                                && let Err(err) = current_devices.lock()?.add_device(
                                    ev.syspath().to_string_lossy().into_owned(),
                                    |name| {
                                        ev.property_value(name)
                                            .map(|value| value.to_string_lossy().into_owned())
                                    },
                                )
                            {
                                log::error!("Couldn't add dev {:?} ({})", ev.syspath(), err);
                            }
                        }
                        udev::EventType::Remove => current_devices.lock()?.rm_device(&ev),
                        _ => (),
                    }
                }
            }
        }
    }
}

/// Decode a property encoded by udev (`ID_VENDOR_ENC`, `ID_MODEL_ENC`):
/// unsafe bytes are written as `\xNN`.
fn udev_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && bytes.get(i + 1) == Some(&b'x')
            && let Some(byte) = bytes
                .get(i + 2..i + 4)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            decoded.push(byte);
            i += 4;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Port path of a device (bus number then ports, e.g. `[1, 1, 2]`), from its
/// bus number and the last component of its `DEVPATH` (e.g. `.../usb1/1-1.2`).
fn port_path(busnum: u8, devpath: &str) -> Result<Vec<u8>> {
    let (_, ports) = devpath
        .rsplit('/')
        .next()
        .and_then(|name| name.split_once('-'))
        .ok_or(Error::NoneValue)?;
    let mut dev_path = vec![busnum];
    for port in ports.split('.') {
        dev_path.push(port.parse::<u8>()?);
    }
    Ok(dev_path)
}

/// Plugged devices, shared between the udev thread and the main thread.
pub struct CurrentDevices {
    /// Devices by sysfs path
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

    /// Add a device from its sysfs path and properties, unless it's plugged on a port that is
    /// neither a source nor a destination port.
    fn add_device(&mut self, syspath: String, prop: impl Fn(&str) -> Option<String>) -> Result<()> {
        if let Some(dev) = usb_device_from_props(prop, &self.usb_port_accesses)? {
            self.devices.insert(syspath, dev);
        }
        Ok(())
    }

    fn rm_device(&mut self, device: &udev::Device) {
        if let Some(dev) = self
            .devices
            .remove(&device.syspath().to_string_lossy().to_string())
        {
            info!("Device unplugged {}-{}", dev.busnum, dev.devnum);
        }
    }
}

/// Build a device from its udev properties.
/// Returns `None` if the device is plugged on a port that is neither a source nor
/// a destination port.
fn usb_device_from_props(
    prop: impl Fn(&str) -> Option<String>,
    usb_port_accesses: &Option<UsbPortAccesses>,
) -> Result<Option<UsbDevice>> {
    let busnum = prop("BUSNUM").ok_or(Error::NoneValue)?.parse::<u8>()?;
    let dev_path = port_path(busnum, &prop("DEVPATH").ok_or(Error::NoneValue)?)?;

    // Check if device is connected to an allowed port if policy configured
    let (is_src, mut is_dst) = if let Some(config) = usb_port_accesses {
        (
            if let Some(ref ports) = config.ports_src {
                ports.contains(&dev_path)
            } else {
                true
            },
            if let Some(ref ports) = config.ports_dst {
                ports.contains(&dev_path)
            } else {
                true
            },
        )
    } else {
        (true, true)
    };

    // Never destination if optical disk reader
    if prop("ID_USB_INTERFACES")
        .ok_or(Error::NoneValue)?
        .contains(":080250:")
    {
        is_dst = false;
    }

    if !is_src && !is_dst {
        log::warn!(
            "Ignoring device plugged in unauthorized port {:?}",
            dev_path.as_slice()
        );
        return Ok(None);
    }

    let dev = UsbDevice {
        busnum: busnum.into(),
        devnum: prop("DEVNUM").ok_or(Error::NoneValue)?.parse::<u32>()?,
        vendorid: u32::from_str_radix(&prop("ID_VENDOR_ID").unwrap_or("0".into()), 16)?,
        productid: u32::from_str_radix(&prop("ID_MODEL_ID").unwrap_or("0".into()), 16)?,
        manufacturer: prop("ID_VENDOR_ENC").map_or("unknown".into(), |v| udev_decode(&v)),
        description: prop("ID_MODEL_ENC").map_or("unknown".into(), |v| udev_decode(&v)),
        serial: prop("ID_SERIAL_SHORT").unwrap_or("unknown".into()),
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

    Ok(Some(dev))
}

/// States of the process, see the module documentation.
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

        // Read the configuration now, parse it once sandboxed
        let config_str = conf_read(&self.config_path)?;

        // The main thread frees the devices sent by the scan thread, the monitor thread frees
        // unplugged devices.
        // libc tries to open '/proc/sys/vm/overcommit_memory' otherwise.
        usbsas_sandbox::single_malloc_arena()?;

        usbsas_sandbox::landlock(
            Some(&["/sys/bus", "/sys/class", "/sys/devices", "/run/udev"]),
            None,
            None,
            None,
            None,
        )?;

        let current_devices = Arc::new(Mutex::new(CurrentDevices::new(None)));
        let cur_dev_clone = current_devices.clone();
        let (listening_tx, listening_rx) = mpsc::sync_channel(1);
        let (start_tx, start_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            if let Err(err) = handle_udev_events(cur_dev_clone, listening_tx, start_rx) {
                error!("monitor thread: {err}");
            }
        });
        listening_rx
            .recv()
            .map_err(|_| Error::Error("monitor thread exited".into()))?;

        let (scanned_tx, scanned_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            if let Err(err) = scan_thread(scanned_tx) {
                error!("scan thread: {err}");
            }
        });

        // No file system access anymore
        usbsas_sandbox::landlock(None, None, None, None, None)?;
        usbsas_sandbox::usbdev::seccomp(comm.input_fd(), comm.output_fd())?;

        let config = conf_parse(&config_str)?;
        if let Some(usb_ports) = &config.usb_port_accesses {
            log::debug!("usb_port_accesses: {usb_ports:?}");
        };

        // Without the scan, only devices plugged from now on are listed
        let scanned = scanned_rx.recv().unwrap_or_else(|_| {
            log::warn!("devices already plugged couldn't be listed");
            Vec::new()
        });
        let mut cur_dev = current_devices.lock()?;
        cur_dev.usb_port_accesses = config.usb_port_accesses;
        for (syspath, props) in scanned {
            if let Err(err) = cur_dev.add_device(syspath.clone(), |name| props.get(name).cloned()) {
                error!("Couldn't add dev {syspath} ({err})");
            }
        }
        drop(cur_dev);
        start_tx
            .send(())
            .map_err(|_| Error::Error("monitor thread exited".into()))?;

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

/// USB devices listing process.
pub struct UsbDev {
    comm: ComRpUsbDev,
    state: State,
}

impl UsbDev {
    /// New lister, `config_path` is the configuration file (for
    /// `usb_port_accesses`).
    pub fn new(comm: ComRpUsbDev, config_path: String) -> Result<Self> {
        Ok(UsbDev {
            comm,
            state: State::Init(InitState { config_path }),
        })
    }

    /// Run the process until it receives `End`. On error, the error is sent to
    /// the parent and the process waits for `End`.
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

#[cfg(test)]
mod tests {
    //! Unit tests of the device parsing (from udev properties) and of the
    //! main thread's states, driven over pipes as usbsas does. The udev thread
    //! and the sandbox are tested with the real binary in `tests/`.

    use super::*;
    use std::{io, os::fd::OwnedFd};
    use usbsas_comm::{ComRqUsbDev, FromFd, ProtoReqCommon, ProtoReqUsbDev};
    use usbsas_proto::usbdev::RequestDevices;

    /// Properties of a USB key on port 1-1.2, as sent by udevd, with `set`
    /// added or replaced and `unset` removed.
    fn props(set: &[(&str, &str)], unset: &[&str]) -> impl Fn(&str) -> Option<String> {
        let mut map: HashMap<String, String> = [
            ("BUSNUM", "001"),
            ("DEVNUM", "021"),
            ("DEVPATH", "/devices/pci0000:00/0000:00:14.0/usb1/1-1/1-1.2"),
            ("ID_USB_INTERFACES", ":080650:"),
            ("ID_VENDOR_ID", "0781"),
            ("ID_MODEL_ID", "5581"),
            ("ID_VENDOR_ENC", "SanDisk\\x27s"),
            ("ID_MODEL_ENC", "Ultra\\x20USB\\x203.0"),
            ("ID_SERIAL_SHORT", "4C530001"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        for (k, v) in set {
            map.insert(k.to_string(), v.to_string());
        }
        for k in unset {
            map.remove(*k);
        }
        move |name| map.get(name).cloned()
    }

    fn device(set: &[(&str, &str)], unset: &[&str]) -> UsbDevice {
        usb_device_from_props(props(set, unset), &None)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn decode() {
        assert_eq!(udev_decode("Ultra\\x20USB\\x203.0"), "Ultra USB 3.0");
        assert_eq!(udev_decode("\\xc3\\xa9"), "é");
        assert_eq!(udev_decode("\\x5cx41"), "\\x41");
        // Not an escape: kept as is
        assert_eq!(udev_decode("a\\x2"), "a\\x2");
        assert_eq!(udev_decode("a\\xzz\\"), "a\\xzz\\");
        assert_eq!(udev_decode("a\\y"), "a\\y");
        assert_eq!(udev_decode(""), "");
        // Invalid UTF-8
        assert_eq!(udev_decode("a\\xffb"), "a\u{fffd}b");
    }

    #[test]
    fn ports() {
        assert_eq!(port_path(1, "/devices/x/usb1/1-10").unwrap(), [1, 10]);
        assert_eq!(
            port_path(3, "/devices/x/usb3/3-1/3-1.4/3-1.4.2").unwrap(),
            [3, 1, 4, 2]
        );
        // Root hub
        assert!(port_path(1, "/devices/x/usb1").is_err());
        assert!(port_path(1, "").is_err());
        assert!(port_path(1, "/devices/x/usb1/1-").is_err());
        assert!(port_path(1, "/devices/x/usb1/1-a").is_err());
        assert!(port_path(1, "/devices/x/usb1/1-1..2").is_err());
        assert!(port_path(1, "/devices/x/usb1/1-1.256").is_err());
    }

    #[test]
    fn device_props() {
        let dev = device(&[], &[]);
        assert_eq!((dev.busnum, dev.devnum), (1, 21));
        assert_eq!((dev.vendorid, dev.productid), (0x0781, 0x5581));
        assert_eq!(dev.manufacturer, "SanDisk's");
        assert_eq!(dev.description, "Ultra USB 3.0");
        assert_eq!(dev.serial, "4C530001");
        assert_eq!((dev.dev_size, dev.block_size), (None, None));
        assert!(dev.is_src && dev.is_dst);

        // Optional properties
        let dev = device(
            &[],
            &[
                "ID_VENDOR_ID",
                "ID_MODEL_ID",
                "ID_VENDOR_ENC",
                "ID_MODEL_ENC",
                "ID_SERIAL_SHORT",
            ],
        );
        assert_eq!((dev.vendorid, dev.productid), (0, 0));
        assert_eq!(dev.manufacturer, "unknown");
        assert_eq!(dev.description, "unknown");
        assert_eq!(dev.serial, "unknown");

        // Optical disk readers are never destinations
        let dev = device(&[("ID_USB_INTERFACES", ":080250:")], &[]);
        assert!(dev.is_src && !dev.is_dst);
        let dev = device(&[("ID_USB_INTERFACES", ":080650:080250:")], &[]);
        assert!(dev.is_src && !dev.is_dst);
    }

    #[test]
    fn device_props_errors() {
        for name in ["BUSNUM", "DEVNUM", "DEVPATH", "ID_USB_INTERFACES"] {
            assert!(
                usb_device_from_props(props(&[], &[name]), &None).is_err(),
                "missing {name}"
            );
        }
        for (name, value) in [
            ("BUSNUM", "256"),
            ("BUSNUM", ""),
            ("DEVNUM", "-1"),
            ("DEVNUM", "x"),
            ("DEVPATH", "/devices/x/usb1"),
            ("DEVPATH", "/devices/x/usb1/1-1.x"),
            ("ID_VENDOR_ID", "zz"),
            ("ID_MODEL_ID", "123456789"),
        ] {
            assert!(
                usb_device_from_props(props(&[(name, value)], &[]), &None).is_err(),
                "{name}={value}"
            );
        }
    }

    #[test]
    fn port_policy() {
        let policy = |src: Option<Vec<Vec<u8>>>, dst: Option<Vec<Vec<u8>>>| {
            Some(UsbPortAccesses {
                ports_src: src,
                ports_dst: dst,
                ports_hid: None,
            })
        };
        let on_port = |port: &str, interfaces: &str, accesses: &Option<UsbPortAccesses>| {
            let devpath = format!("/devices/x/usb1/{port}");
            usb_device_from_props(
                props(
                    &[("DEVPATH", &devpath), ("ID_USB_INTERFACES", interfaces)],
                    &[],
                ),
                accesses,
            )
            .unwrap()
            .map(|dev| (dev.is_src, dev.is_dst))
        };
        let key = ":080650:";
        let optical = ":080250:";

        let accesses = policy(Some(vec![vec![1, 1, 2]]), Some(vec![vec![1, 3]]));
        assert_eq!(on_port("1-1.2", key, &accesses), Some((true, false)));
        assert_eq!(on_port("1-3", key, &accesses), Some((false, true)));
        assert_eq!(on_port("1-4", key, &accesses), None);
        // Same port numbers on another bus
        assert_eq!(
            usb_device_from_props(props(&[("BUSNUM", "2")], &[]), &accesses).unwrap(),
            None
        );
        // Prefix of an allowed port
        assert_eq!(on_port("1-1", key, &accesses), None);
        // Optical disk reader on a destination port
        assert_eq!(on_port("1-3", optical, &accesses), None);
        assert_eq!(on_port("1-1.2", optical, &accesses), Some((true, false)));

        // Unset lists allow every port
        let accesses = policy(Some(vec![vec![1, 1, 2]]), None);
        assert_eq!(on_port("1-1.2", key, &accesses), Some((true, true)));
        assert_eq!(on_port("1-4", key, &accesses), Some((false, true)));
        let accesses = policy(None, Some(vec![vec![1, 3]]));
        assert_eq!(on_port("1-4", key, &accesses), Some((true, false)));
        assert_eq!(on_port("1-3", optical, &accesses), Some((true, false)));
        let accesses = policy(None, None);
        assert_eq!(on_port("1-4", key, &accesses), Some((true, true)));

        // Empty lists allow none
        let accesses = policy(Some(vec![]), Some(vec![]));
        assert_eq!(on_port("1-1.2", key, &accesses), None);
    }

    /// `state` running in a thread, driven over pipes. The thread returns
    /// whether the state ended with `End`.
    fn spawn_state(state: State) -> (ComRqUsbDev, thread::JoinHandle<bool>) {
        let (to_child_rd, to_child_wr) = io::pipe().unwrap();
        let (to_parent_rd, to_parent_wr) = io::pipe().unwrap();
        let mut rp = ComRpUsbDev::from_fd(OwnedFd::from(to_child_rd), OwnedFd::from(to_parent_wr));
        let rq = ComRqUsbDev::from_fd(OwnedFd::from(to_parent_rd), OwnedFd::from(to_child_wr));
        let handle = thread::spawn(move || matches!(state.run(&mut rp), Ok(State::End)));
        (rq, handle)
    }

    #[test]
    fn running_state() {
        let current_devices = Arc::new(Mutex::new(CurrentDevices::new(None)));
        let (mut rq, handle) = spawn_state(State::Running(RunningState {
            current_devices: current_devices.clone(),
        }));

        assert!(rq.devices(RequestDevices {}).unwrap().devices.is_empty());

        // Devices added by the udev thread are listed
        let dev = device(&[], &[]);
        current_devices
            .lock()
            .unwrap()
            .devices
            .insert("/sys/devices/x/usb1/1-1/1-1.2".into(), dev.clone());
        assert_eq!(rq.devices(RequestDevices {}).unwrap().devices, [dev]);

        current_devices.lock().unwrap().devices.clear();
        assert!(rq.devices(RequestDevices {}).unwrap().devices.is_empty());

        rq.end().unwrap();
        assert!(handle.join().unwrap());
    }

    #[test]
    fn wait_end_state() {
        let (mut rq, handle) = spawn_state(State::WaitEnd(WaitEndState {}));
        let err = rq.devices(RequestDevices {}).unwrap_err();
        assert_eq!(err.to_string(), "bad request");
        rq.end().unwrap();
        assert!(handle.join().unwrap());
    }
}
