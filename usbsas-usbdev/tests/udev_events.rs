//! Integration test: the real `usbsas-usbdev` binary, Landlock and seccomp
//! included, receives udev events forged by the test and is queried over its
//! pipes as usbsas does.
//!
//! udevd sends its events to a netlink multicast group, sending to it needs
//! `CAP_NET_ADMIN`. The test enters new user, network and mount namespaces
//! (where it has it), and mounts a tmpfs on `/run` with an empty
//! `/run/udev/control` (libudev doesn't listen to udevd's events without it).
//! It doesn't depend on the host's udev nor its devices. It is skipped if the
//! namespaces can't be created (e.g. unprivileged user namespaces disabled, or
//! in a container).
//!
//! `unshare(CLONE_NEWUSER)` needs a single-threaded process: this test has its
//! own `main` (`harness = false`) and runs its cases sequentially.

use std::{
    ffi::CString,
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::process::{CommandExt, ExitStatusExt},
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};
use usbsas_comm::{ComRqUsbDev, FromFd, ProtoReqCommon, ProtoReqUsbDev};
use usbsas_proto::{common::UsbDevice, usbdev::RequestDevices};
use usbsas_utils::{INPUT_PIPE_FD_VAR, OUTPUT_PIPE_FD_VAR};

/// udevd's netlink multicast group (the kernel's is 1).
const UDEV_GROUP: u32 = 2;
const UDEV_MONITOR_MAGIC: u32 = 0xfeed_cafe;
/// Time allowed for usbdev to handle events.
const TIMEOUT: Duration = Duration::from_secs(10);

/// sysfs path of a device on the first controller, `name` is the last
/// component (e.g. `1-1.2`, or `usb1` for the root hub).
fn devpath(name: &str) -> String {
    format!("/devices/pci0000:00/0000:00:14.0/usb1/{name}")
}

fn syscall_result(ret: libc::c_int) -> io::Result<()> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Enter new user (mapped to root), network and mount namespaces, and mount
/// a tmpfs on `/run` with an empty `/run/udev/control`.
fn enter_namespaces() -> io::Result<()> {
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    syscall_result(unsafe {
        libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET | libc::CLONE_NEWNS)
    })?;
    fs::write("/proc/self/setgroups", "deny")?;
    fs::write("/proc/self/uid_map", format!("0 {uid} 1"))?;
    fs::write("/proc/self/gid_map", format!("0 {gid} 1"))?;

    let root = CString::new("/").unwrap();
    let run = CString::new("/run").unwrap();
    let tmpfs = CString::new("tmpfs").unwrap();
    syscall_result(unsafe {
        libc::mount(
            std::ptr::null(),
            root.as_ptr(),
            std::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            std::ptr::null(),
        )
    })?;
    syscall_result(unsafe {
        libc::mount(
            tmpfs.as_ptr(),
            run.as_ptr(),
            tmpfs.as_ptr(),
            0,
            std::ptr::null(),
        )
    })?;
    fs::create_dir("/run/udev")?;
    fs::write("/run/udev/control", "")?;
    Ok(())
}

/// MurmurHash2 (seed 0), used by libudev to hash the subsystem and device
/// type in its messages header (filtered by the monitor's socket filter).
fn murmur2(data: &[u8]) -> u32 {
    const M: u32 = 0x5bd1_e995;
    let mut h = data.len() as u32;
    let (chunks, rest) = data.as_chunks::<4>();
    for chunk in chunks {
        let mut k = u32::from_le_bytes(*chunk);
        k = k.wrapping_mul(M);
        k ^= k >> 24;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M) ^ k;
    }
    if rest.len() >= 3 {
        h ^= u32::from(rest[2]) << 16;
    }
    if rest.len() >= 2 {
        h ^= u32::from(rest[1]) << 8;
    }
    if !rest.is_empty() {
        h ^= u32::from(rest[0]);
        h = h.wrapping_mul(M);
    }
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^ (h >> 15)
}

/// Sends udev events as udevd does (libudev message on the udev group).
struct UdevSender {
    sock: OwnedFd,
    seqnum: u64,
}

impl UdevSender {
    fn new() -> io::Result<Self> {
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                libc::NETLINK_KOBJECT_UEVENT,
            )
        };
        syscall_result(fd)?;
        let sock = unsafe { OwnedFd::from_raw_fd(fd) };
        let addr = Self::addr(0);
        syscall_result(unsafe {
            libc::bind(
                sock.as_raw_fd(),
                (&raw const addr).cast(),
                size_of::<libc::sockaddr_nl>() as u32,
            )
        })?;
        Ok(UdevSender { sock, seqnum: 0 })
    }

    fn addr(groups: u32) -> libc::sockaddr_nl {
        let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        addr.nl_family = libc::AF_NETLINK as u16;
        addr.nl_groups = groups;
        addr
    }

    /// Send an event of a `usb_device` with the given `ACTION`, `DEVPATH`
    /// and other properties.
    fn send(&mut self, action: &str, devpath: &str, props: &[(&str, &str)]) {
        self.send_devtype(action, devpath, "usb_device", props)
    }

    fn send_devtype(&mut self, action: &str, devpath: &str, devtype: &str, props: &[(&str, &str)]) {
        self.seqnum += 1;
        let mut body = Vec::new();
        for (key, value) in [
            ("ACTION", action),
            ("DEVPATH", devpath),
            ("SUBSYSTEM", "usb"),
            ("DEVTYPE", devtype),
            ("SEQNUM", &self.seqnum.to_string()),
            ("USEC_INITIALIZED", "1"),
        ]
        .iter()
        .chain(props)
        {
            body.extend_from_slice(format!("{key}={value}\0").as_bytes());
        }

        // struct monitor_netlink_header (sd-device-monitor.c)
        const HEADER_SIZE: u32 = 40;
        let mut msg = b"libudev\0".to_vec();
        msg.extend_from_slice(&UDEV_MONITOR_MAGIC.to_be_bytes());
        msg.extend_from_slice(&HEADER_SIZE.to_ne_bytes());
        msg.extend_from_slice(&HEADER_SIZE.to_ne_bytes()); // properties offset
        msg.extend_from_slice(&(body.len() as u32).to_ne_bytes());
        msg.extend_from_slice(&murmur2(b"usb").to_be_bytes());
        msg.extend_from_slice(&murmur2(devtype.as_bytes()).to_be_bytes());
        msg.extend_from_slice(&[0; 8]); // tags bloom filter
        assert_eq!(msg.len(), HEADER_SIZE as usize);
        msg.extend_from_slice(&body);

        let addr = Self::addr(UDEV_GROUP);
        let ret = unsafe {
            libc::sendto(
                self.sock.as_raw_fd(),
                msg.as_ptr().cast(),
                msg.len(),
                0,
                (&raw const addr).cast(),
                size_of::<libc::sockaddr_nl>() as u32,
            )
        };
        assert_eq!(ret, msg.len() as isize, "{}", io::Error::last_os_error());
    }

    /// Mass storage device (SCSI bulk only) plugged on port `name` (e.g.
    /// `1-1.2`) of bus 1.
    fn add_key(&mut self, name: &str, devnum: &str, serial: &str) {
        self.send(
            "add",
            &devpath(name),
            &[
                ("BUSNUM", "001"),
                ("DEVNUM", devnum),
                ("ID_USB_INTERFACES", ":080650:"),
                ("ID_VENDOR_ID", "0781"),
                ("ID_MODEL_ID", "5581"),
                ("ID_VENDOR_ENC", "San\\x20Disk"),
                ("ID_MODEL_ENC", "Ultra\\x20USB\\x203.0"),
                ("ID_SERIAL_SHORT", serial),
            ],
        );
    }

    fn remove(&mut self, name: &str) {
        self.send("remove", &devpath(name), &[]);
    }
}

/// Temporary configuration file, removed on drop.
struct TmpConfig(PathBuf);

impl TmpConfig {
    fn new(content: &str) -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "usbsas-usbdev-test-{}-{}.toml",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::write(&path, content).unwrap();
        TmpConfig(path)
    }
}

impl Drop for TmpConfig {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// usbdev spawned as usbsas does (pipes passed in the environment).
struct UsbDev {
    child: Child,
    comm: ComRqUsbDev,
    _config: TmpConfig,
}

impl UsbDev {
    fn spawn(config: TmpConfig) -> Self {
        let (to_child_rd, to_child_wr) = io::pipe().unwrap();
        let (to_parent_rd, to_parent_wr) = io::pipe().unwrap();
        let child_fds: [RawFd; 2] = [to_child_rd.as_raw_fd(), to_parent_wr.as_raw_fd()];

        let mut command = Command::new(env!("CARGO_BIN_EXE_usbsas-usbdev"));
        command
            .arg("--config")
            .arg(&config.0)
            .env_clear()
            .env(INPUT_PIPE_FD_VAR, child_fds[0].to_string())
            .env(OUTPUT_PIPE_FD_VAR, child_fds[1].to_string())
            .stdin(Stdio::null());
        if let Ok(log) = std::env::var("RUST_LOG") {
            command.env("RUST_LOG", log);
        }
        // SAFETY: fcntl is async-signal-safe.
        unsafe {
            command.pre_exec(move || {
                for fd in child_fds {
                    syscall_result(libc::fcntl(fd, libc::F_SETFD, 0))?;
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        drop((to_child_rd, to_parent_wr));

        UsbDev {
            child,
            comm: ComRqUsbDev::from_fd(OwnedFd::from(to_parent_rd), OwnedFd::from(to_child_wr)),
            _config: config,
        }
    }

    fn devices(&mut self) -> Vec<UsbDevice> {
        let mut devices = match self.comm.devices(RequestDevices {}) {
            Ok(resp) => resp.devices,
            // Killed by seccomp, for instance
            Err(err) => panic!(
                "Devices request failed ({err}), usbdev: {:?}",
                self.child.wait()
            ),
        };
        devices.sort_by_key(|dev| dev.devnum);
        devices
    }

    /// Wait until `cond` is true for the listed devices.
    fn wait_devices(&mut self, what: &str, cond: impl Fn(&[UsbDevice]) -> bool) -> Vec<UsbDevice> {
        let start = Instant::now();
        loop {
            let devices = self.devices();
            if cond(&devices) {
                return devices;
            }
            assert!(
                start.elapsed() < TIMEOUT,
                "timeout waiting for {what}, devices: {devices:#?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Wait until the udev thread listens: re-send the event of a key on
    /// `name` until it is listed (events sent before are lost).
    fn wait_listening(&mut self, udev: &mut UdevSender, name: &str, devnum: &str) {
        let start = Instant::now();
        loop {
            udev.add_key(name, devnum, "SYNC");
            thread::sleep(Duration::from_millis(20));
            if self.devices().iter().any(|dev| dev.serial == "SYNC") {
                return;
            }
            assert!(start.elapsed() < TIMEOUT, "udev thread not listening");
        }
    }

    /// End the process and check it exited successfully.
    fn end(mut self) {
        self.comm.end().unwrap();
        self.wait_success();
    }

    /// Check the process exited successfully (in particular, that it wasn't
    /// killed by seccomp).
    fn wait_success(mut self) {
        let status = self.child.wait().unwrap();
        assert!(
            status.success(),
            "usbdev exited with {status:?} (signal {:?})",
            status.signal()
        );
    }
}

/// Serial numbers of `devices`.
fn serials(devices: &[UsbDevice]) -> Vec<&str> {
    devices.iter().map(|dev| dev.serial.as_str()).collect()
}

fn has_serial(devices: &[UsbDevice], serial: &str) -> bool {
    devices.iter().any(|dev| dev.serial == serial)
}

/// Devices added and removed, malformed or ignored events.
fn events(udev: &mut UdevSender) {
    let mut usbdev = UsbDev::spawn(TmpConfig::new(""));
    usbdev.wait_listening(udev, "1-9", "90");
    udev.remove("1-9");
    usbdev.wait_devices("sync device removed", |devs| devs.is_empty());

    udev.add_key("1-1/1-1.2", "21", "KEY");
    // Optical disk reader, without optional properties
    udev.send(
        "add",
        &devpath("1-3"),
        &[
            ("BUSNUM", "001"),
            ("DEVNUM", "5"),
            ("ID_USB_INTERFACES", ":080250:"),
        ],
    );
    // Not mass storage
    udev.send(
        "add",
        &devpath("1-4"),
        &[
            ("BUSNUM", "001"),
            ("DEVNUM", "6"),
            ("ID_USB_INTERFACES", ":030000:"),
            ("ID_SERIAL_SHORT", "HID"),
        ],
    );
    // Mass storage interface (not a usb_device): filtered by the monitor
    udev.send_devtype(
        "add",
        &devpath("1-5/1-5:1.0"),
        "usb_interface",
        &[
            ("BUSNUM", "001"),
            ("DEVNUM", "7"),
            ("ID_USB_INTERFACES", ":080650:"),
            ("ID_SERIAL_SHORT", "INTERFACE"),
        ],
    );
    // Malformed: no port (root hub), missing or invalid properties
    for (name, props) in [
        ("usb1", vec![("BUSNUM", "001"), ("DEVNUM", "1")]),
        ("1-6", vec![("DEVNUM", "8")]),
        ("1-6", vec![("BUSNUM", "001")]),
        ("1-6", vec![("BUSNUM", "300"), ("DEVNUM", "8")]),
        ("1-x", vec![("BUSNUM", "001"), ("DEVNUM", "8")]),
        (
            "1-6",
            vec![("BUSNUM", "001"), ("DEVNUM", "8"), ("ID_VENDOR_ID", "zz")],
        ),
    ] {
        let props: Vec<_> = props
            .into_iter()
            .chain([
                ("ID_USB_INTERFACES", ":080650:"),
                ("ID_SERIAL_SHORT", "BAD"),
            ])
            .collect();
        udev.send("add", &devpath(name), &props);
    }
    // Other actions are ignored
    udev.send(
        "bind",
        &devpath("1-7"),
        &[
            ("BUSNUM", "001"),
            ("DEVNUM", "9"),
            ("ID_USB_INTERFACES", ":080650:"),
            ("ID_SERIAL_SHORT", "BIND"),
        ],
    );
    // Events are handled in order: once this one is listed, the ones above
    // have been handled.
    udev.add_key("1-8", "99", "LAST");
    let devices = usbdev.wait_devices("last device", |devs| has_serial(devs, "LAST"));
    assert_eq!(serials(&devices), ["unknown", "KEY", "LAST"]);

    let optical = &devices[0];
    assert_eq!((optical.busnum, optical.devnum), (1, 5));
    assert_eq!((optical.vendorid, optical.productid), (0, 0));
    assert_eq!(optical.manufacturer, "unknown");
    assert_eq!(optical.description, "unknown");
    assert!(optical.is_src && !optical.is_dst);

    let key = &devices[1];
    assert_eq!((key.busnum, key.devnum), (1, 21));
    assert_eq!((key.vendorid, key.productid), (0x0781, 0x5581));
    assert_eq!(key.manufacturer, "San Disk");
    assert_eq!(key.description, "Ultra USB 3.0");
    assert_eq!((key.dev_size, key.block_size), (None, None));
    assert!(key.is_src && key.is_dst);

    // Unplug, unplug of an unknown device, plug again
    udev.remove("1-1/1-1.2");
    udev.remove("1-10");
    udev.remove("1-8");
    let devices = usbdev.wait_devices("devices removed", |devs| !has_serial(devs, "LAST"));
    assert_eq!(serials(&devices), ["unknown"]);
    udev.add_key("1-1/1-1.2", "22", "KEY");
    let devices = usbdev.wait_devices("device plugged again", |devs| devs.len() == 2);
    assert_eq!(devices[1].devnum, 22);

    usbdev.end();
}

/// Source and destination ports (`usb_port_accesses`).
fn port_policy(udev: &mut UdevSender) {
    let mut usbdev = UsbDev::spawn(TmpConfig::new(
        "[usb_port_accesses]\nports_src = [ [1, 1, 2], [1, 9] ]\nports_dst = [ [1, 3], [1, 9] ]\n",
    ));
    usbdev.wait_listening(udev, "1-9", "90");
    udev.remove("1-9");
    usbdev.wait_devices("sync device removed", |devs| devs.is_empty());

    udev.add_key("1-1/1-1.2", "1", "SRC");
    udev.add_key("1-3", "2", "DST");
    udev.add_key("1-4", "3", "NONE");
    udev.add_key("1-1", "4", "PREFIX");
    udev.send(
        "add",
        &devpath("1-3"),
        &[
            ("BUSNUM", "001"),
            ("DEVNUM", "5"),
            ("ID_USB_INTERFACES", ":080250:"),
            ("ID_SERIAL_SHORT", "OPTICAL_DST"),
        ],
    );
    udev.add_key("1-9", "99", "LAST");
    let devices = usbdev.wait_devices("last device", |devs| has_serial(devs, "LAST"));
    assert_eq!(serials(&devices), ["SRC", "DST", "LAST"]);
    assert!(devices[0].is_src && !devices[0].is_dst);
    assert!(!devices[1].is_src && devices[1].is_dst);
    assert!(devices[2].is_src && devices[2].is_dst);

    usbdev.end();
}

/// An invalid configuration is reported to the parent (parsed once
/// sandboxed), the process still ends successfully.
fn invalid_config() {
    let mut usbdev = UsbDev::spawn(TmpConfig::new("this is [not toml\n"));
    let err = usbdev.comm.devices(RequestDevices {}).unwrap_err();
    assert!(
        err.to_string().contains("Couldn't parse config"),
        "unexpected error: {err}"
    );
    // `Init` sends its error without waiting for a request, so it was read
    // as the response to the request above. The actual response to that
    // request ("bad request", from `WaitEnd`) is read next: responses are one
    // behind, `End` gets it.
    let err = usbdev.comm.end().unwrap_err();
    assert_eq!(err.to_string(), "bad request");
    usbdev.wait_success();
}

fn main() {
    if let Err(err) = enter_namespaces() {
        println!("udev_events: skipped, can't set up namespaces: {err}");
        return;
    }
    let mut udev = UdevSender::new().expect("netlink socket");

    type Test<'a> = (&'a str, &'a dyn Fn(&mut UdevSender));
    let tests: [Test; 3] = [
        ("events", &events),
        ("port_policy", &port_policy),
        ("invalid_config", &|_| invalid_config()),
    ];
    for (name, test) in tests {
        print!("test {name} ... ");
        test(&mut udev);
        println!("ok");
    }
}
