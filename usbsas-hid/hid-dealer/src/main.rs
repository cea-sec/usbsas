use dbus::blocking::Connection;
use dbus_crossroads::{Context, Crossroads};
use std::{
    collections::HashMap,
    error::Error,
    process::{Child, Command},
    sync::Mutex,
    {thread, time},
};

lazy_static::lazy_static! {
    static ref HM_SONS: Mutex<HashMap<(u8, u8), Child>> = {
        let hm = HashMap::new();
        Mutex::new(hm)
    };
}

struct Device {}

fn run_son(busnum: u8, devnum: u8) -> Result<Child, std::io::Error> {
    let mut filtered_env: HashMap<String, String> = std::env::vars()
        .filter(|&(ref k, _)| {
            k == "TERM"
                || k == "LANG"
                || k == "HOME"
                || k == "PATH"
                || k == "DISPLAY"
                || k == "RUST_LOG"
                || k == "RUST_BACKTRACE"
        })
        .collect();

    filtered_env.insert("BUSNUM".to_owned(), format!("{}", busnum));
    filtered_env.insert("DEVNUM".to_owned(), format!("{}", devnum));

    Command::new("/usr/libexec/hid-user")
        .env_clear()
        .envs(&filtered_env)
        .spawn()
}

fn update_entry(
    _: &mut Context,
    _: &mut Device,
    (busnum, devnum): (u8, u8),
) -> Result<(), dbus_crossroads::MethodErr> {
    log::debug!("Incoming update for {} {}", busnum, devnum);
    let mut sons = HM_SONS.lock().unwrap();
    let key = (busnum, devnum);
    if let Some(mut child) = sons.remove(&key) {
        log::info!("killing client for {} {}", busnum, devnum);
        child.kill().expect("Cannot kill son");
        let result = child.wait();
        log::debug!("wait: {:?}", result);
    }

    match run_son(busnum, devnum) {
        Ok(child) => {
            sons.insert(key, child);
        }
        Err(_) => {
            log::error!("Cannot run son");
        }
    }
    Ok(())
}

fn remove_entry(
    _: &mut Context,
    _: &mut Device,
    (busnum, devnum): (u8, u8),
) -> Result<(), dbus_crossroads::MethodErr> {
    log::debug!("Incoming remove for {} {}!", busnum, devnum);

    let mut sons = HM_SONS.lock().unwrap();
    let key = (busnum, devnum);
    if let Some(mut child) = sons.remove(&key) {
        log::info!("killing client for {} {}", busnum, devnum);
        child.kill().expect("Cannot kill son");
        let result = child.wait();
        log::debug!("wait: {:?}", result);
    } else {
        log::error!("Unknown client {} {}", busnum, devnum)
    }

    Ok(())
}

fn wait_sons() -> ! {
    loop {
        {
            let mut sons = HM_SONS.lock().unwrap();
            sons.retain(|&_, child| match child.try_wait() {
                Ok(Some(status)) => {
                    log::info!("Son {:?} ended with status {}", child.id(), status);
                    false
                }
                Ok(None) => true,
                Err(err) => {
                    log::error!("Wait son {:?} error {:?}", child.id(), err);
                    false
                }
            })
        }
        let wait_time = time::Duration::from_millis(1000);
        thread::sleep(wait_time);
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::Builder::from_default_env()
        .target(env_logger::Target::Stdout)
        .init();

    let c = Connection::new_session()?;
    c.request_name("usbsas.hid", false, true, false)?;

    thread::spawn(move || {
        wait_sons();
    });

    let mut cr = Crossroads::new();

    let iface_token = cr.register("usbsas.hid", |b| {
        b.signal::<(String,), _>("HelloHappened", ("sender",));
        b.method("update", ("busnum", "devnum"), (), update_entry);
        b.method("remove", ("busnum", "devnum"), (), remove_entry);
    });

    cr.insert("/usb_device", &[iface_token], Device {});

    // Serve clients forever.
    cr.serve(&c)?;
    unreachable!()
}
