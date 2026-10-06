use mio::{Events, Interest, Poll, Token};
use std::{
    collections::HashMap,
    error::Error,
    ffi::OsStr,
    process::{Child, Command},
    sync::{Mutex, mpsc},
    {thread, time},
};
use usbsas_utils::USBSAS_BIN_PATH;

lazy_static::lazy_static! {
    static ref HM_SONS: Mutex<HashMap<(u8, u8), Child>> = {
        let hm = HashMap::new();
        Mutex::new(hm)
    };
}

enum DevEvent {
    Update(u8, u8),
    Remove(u8, u8),
}

fn run_son(busnum: u8, devnum: u8) -> Result<Child, std::io::Error> {
    let mut filtered_env: HashMap<String, String> = std::env::vars()
        .filter(|(k, _)| {
            k == "TERM"
                || k == "LANG"
                || k == "HOME"
                || k == "PATH"
                || k == "RUST_LOG"
                || k == "RUST_BACKTRACE"
        })
        .collect();

    filtered_env.insert("BUSNUM".to_owned(), format!("{busnum}"));
    filtered_env.insert("DEVNUM".to_owned(), format!("{devnum}"));

    Command::new(format!("{USBSAS_BIN_PATH}/hid-user"))
        .env_clear()
        .envs(&filtered_env)
        .spawn()
}

fn update_entry(busnum: u8, devnum: u8) {
    let mut sons = HM_SONS.lock().unwrap();
    let key = (busnum, devnum);
    if let Some(mut child) = sons.remove(&key) {
        log::debug!("killing hid-user for {busnum} {devnum}");
        child.kill().expect("Cannot kill son");
        let result = child.wait();
        log::debug!("wait: {result:?}");
    }

    log::debug!("Run hid-user with {busnum} {devnum}");
    match run_son(busnum, devnum) {
        Ok(child) => {
            sons.insert(key, child);
        }
        Err(err) => {
            log::error!("Cannot run hid-user: {}", err);
        }
    }
}

fn remove_entry(busnum: u8, devnum: u8) {
    log::debug!("Incoming remove for {busnum} {devnum}!");

    let mut sons = HM_SONS.lock().unwrap();
    let key = (busnum, devnum);
    if let Some(mut child) = sons.remove(&key) {
        log::info!("killing client for {busnum} {devnum}");
        child.kill().expect("Cannot kill son");
        let result = child.wait();
        log::debug!("wait: {result:?}");
    } else {
        log::error!("Unknown client {busnum} {devnum}");
    }
}

fn wait_sons() -> ! {
    loop {
        {
            let mut sons = HM_SONS.lock().unwrap();
            sons.retain(|&_, child| match child.try_wait() {
                Ok(Some(status)) => {
                    log::debug!("Son {:?} ended with status {status}", child.id());
                    false
                }
                Ok(None) => true,
                Err(err) => {
                    log::error!("Wait son {:?} error {err:?}", child.id());
                    false
                }
            })
        }
        let wait_time = time::Duration::from_millis(1000);
        thread::sleep(wait_time);
    }
}

fn busnum_devnum_from_hid_dev(device: &udev::Device) -> Option<(u8, u8)> {
    let id_usb_interfaces = device.property_value("ID_USB_INTERFACES")?;
    // Check device is HID
    if !id_usb_interfaces
        .to_string_lossy()
        .split(':')
        .any(|iface| iface.get(0..2) == Some("03"))
    {
        return None;
    }
    let busnum = device
        .property_value("BUSNUM")?
        .to_string_lossy()
        .parse::<u8>()
        .ok()?;
    let devnum = device
        .property_value("DEVNUM")?
        .to_string_lossy()
        .parse::<u8>()
        .ok()?;
    Some((busnum, devnum))
}

fn dev_info(device: &udev::Device) -> String {
    format!(
        "ProductID: {}, VendorId: {}, Manufacturer: '{}', Product: '{}', Serial: {}",
        u32::from_str_radix(
            &device
                .attribute_value("idVendor")
                .unwrap_or(OsStr::new("0"))
                .to_string_lossy(),
            16,
        )
        .unwrap_or(0),
        u32::from_str_radix(
            &device
                .attribute_value("idProduct")
                .unwrap_or(OsStr::new("0"))
                .to_string_lossy(),
            16,
        )
        .unwrap_or(0),
        device
            .attribute_value("manufacturer")
            .unwrap_or(OsStr::new("unknown"))
            .to_string_lossy()
            .to_string(),
        device
            .attribute_value("product")
            .unwrap_or(OsStr::new("unknown"))
            .to_string_lossy()
            .to_string(),
        device
            .attribute_value("serial")
            .unwrap_or(OsStr::new("unknown"))
            .to_string_lossy()
            .to_string()
    )
}

/// Look for already plugged HID devices then monitor udev events for new ones
fn handle_udev_events(tx: mpsc::Sender<DevEvent>) -> Result<(), Box<dyn Error>> {
    let monitor = udev::MonitorBuilder::new()?.match_subsystem_devtype("usb", "usb_device")?;
    let mut poll = Poll::new()?;

    let mut socket = monitor.listen()?;
    let mut events = Events::with_capacity(1024);

    poll.registry().register(
        &mut socket,
        Token(0),
        Interest::READABLE | Interest::WRITABLE,
    )?;

    // Scan already plugged devices and update the HID ones
    let mut enumerator = udev::Enumerator::new()?;
    enumerator.match_subsystem("usb")?;

    for dev in enumerator.scan_devices()? {
        if let Some((busnum, devnum)) = busnum_devnum_from_hid_dev(&dev) {
            log::info!(
                "HID device already plugged at startup: {} - {} / {}",
                busnum,
                devnum,
                dev_info(&dev)
            );
            tx.send(DevEvent::Update(busnum, devnum))?;
        }
    }

    // Handle udev events
    loop {
        poll.poll(&mut events, None)?;

        for event in &events {
            if event.token() == Token(0) && event.is_writable() {
                for ev in socket.iter() {
                    match ev.event_type() {
                        udev::EventType::Add | udev::EventType::Change => {
                            if let Some((busnum, devnum)) = busnum_devnum_from_hid_dev(&ev.device())
                            {
                                log::info!(
                                    "HID device plugged: {} - {} / {}",
                                    busnum,
                                    devnum,
                                    dev_info(&ev.device())
                                );
                                tx.send(DevEvent::Update(busnum, devnum))?;
                            }
                        }
                        udev::EventType::Remove => {
                            if let Some((busnum, devnum)) = busnum_devnum_from_hid_dev(&ev.device())
                            {
                                log::info!(
                                    "HID device unplugged: {} - {} / {}",
                                    busnum,
                                    devnum,
                                    dev_info(&ev.device())
                                );

                                tx.send(DevEvent::Remove(busnum, devnum))?;
                            }
                        }
                        ev => log::debug!("Unsupported udev event: {ev}"),
                    }
                }
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::Builder::from_default_env()
        .target(env_logger::Target::Stdout)
        .init();

    usbsas_sandbox::landlock(
        Some(&[
            "/proc/",
            "/run/udev",
            "/sys/bus/",
            "/sys/class/",
            "/sys/devices/",
            "/lib",
            "/usr/lib",
            USBSAS_BIN_PATH,
        ]),
        Some(&["/dev/bus/usb", "/dev/uinput"]),
        Some(&[USBSAS_BIN_PATH]),
        None,
        None,
    )?;

    thread::spawn(wait_sons);

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        if let Err(err) = handle_udev_events(tx) {
            log::error!("udev events thread error: {err}");
        }
    });

    for event in rx {
        match event {
            DevEvent::Update(busnum, devnum) => update_entry(busnum, devnum),
            DevEvent::Remove(busnum, devnum) => remove_entry(busnum, devnum),
        }
    }

    Ok(())
}
