//! Simple program that displays plugged & unplugged USB device with their
//! logical path to configure USB ports restrictions in usbsas.

use mio::{Events, Interest, Poll, Token};
use thiserror::Error;

#[derive(Error, Debug)]
enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("ParseInt error {0}")]
    ParseInt(#[from] std::num::ParseIntError),
    #[error("{0}")]
    NoValue(String),
}
type Result<T> = std::result::Result<T, Error>;

fn main() -> Result<()> {
    env_logger::init_from_env(env_logger::Env::default().filter_or("RUST_LOG", "info"));

    let monitor = udev::MonitorBuilder::new()?.match_subsystem_devtype("usb", "usb_device")?;
    let mut poll = Poll::new()?;
    let mut socket = monitor.listen()?;
    let mut events = Events::with_capacity(1024);

    poll.registry().register(
        &mut socket,
        Token(0),
        Interest::READABLE | Interest::WRITABLE,
    )?;

    loop {
        poll.poll(&mut events, None)?;

        for event in &events {
            if event.token() == Token(0) && event.is_writable() {
                for ev in socket.iter() {
                    log::trace!("{:#?}", ev);
                    log::debug!("{:#?}", ev.device());
                    let busnum = ev
                        .device()
                        .property_value("BUSNUM")
                        .ok_or(Error::NoValue("no busnum".to_string()))?
                        .to_string_lossy()
                        .parse::<u8>()?;
                    let devnum = ev
                        .device()
                        .property_value("DEVNUM")
                        .ok_or(Error::NoValue("no devnum".to_string()))?
                        .to_string_lossy()
                        .parse::<u8>()?;

                    match ev.event_type() {
                        udev::EventType::Add => {
                            let dtype = if let Some(value) = ev.property_value("ID_USB_INTERFACES")
                            {
                                if value.to_string_lossy().contains(":080650:") {
                                    "Mass Storage"
                                } else if value
                                    .to_string_lossy()
                                    .split(':')
                                    .any(|iface| iface.get(0..2) == Some("03"))
                                {
                                    "HID"
                                } else {
                                    "unknown"
                                }
                            } else {
                                continue;
                            };
                            let mut devpath = Vec::new();
                            devpath.push(busnum);
                            for port in ev
                                .device()
                                .attribute_value("devpath")
                                .ok_or(Error::NoValue("no devpath".to_string()))?
                                .to_string_lossy()
                                .split('.')
                            {
                                devpath.push(port.parse::<u8>()?)
                            }

                            log::info!(
                                "Add {dtype} device ({busnum}, {devnum}), on USB port: {devpath:?}"
                            );
                        }
                        udev::EventType::Remove => {
                            log::info!("Remove device ({busnum}, {devnum})");
                        }
                        _ => continue,
                    };
                }
            }
        }
    }
}
