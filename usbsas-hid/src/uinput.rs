//! Minimal `uinput` backend.

use std::{
    fs::{File, OpenOptions},
    io::{Error, Write},
    mem::size_of,
    os::unix::io::{AsRawFd, RawFd},
};

/* linux/input-event-codes.h */
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;

pub const SYN_REPORT: u16 = 0x00;

pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const REL_HWHEEL: u16 = 0x06;
pub const REL_WHEEL: u16 = 0x08;

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_MT_SLOT: u16 = 0x2f;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;

pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;
pub const BTN_TOUCH: u16 = 0x14a;

pub const INPUT_PROP_POINTER: u16 = 0x00;
pub const INPUT_PROP_DIRECT: u16 = 0x01;

const BUS_USB: u16 = 0x03;
const UINPUT_MAX_NAME_SIZE: usize = 80;
const UINPUT_PATH: &str = "/dev/uinput";

/* linux/uinput.h ioctls */
const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = 8;
const IOC_SIZESHIFT: u32 = 16;
const IOC_DIRSHIFT: u32 = 30;
const IOC_WRITE: u32 = 1;
const UINPUT_IOCTL_BASE: u32 = b'U' as u32;

const fn io(nr: u32) -> u32 {
    (UINPUT_IOCTL_BASE << IOC_TYPESHIFT) | (nr << IOC_NRSHIFT)
}

const fn iow(nr: u32, size: usize) -> u32 {
    (IOC_WRITE << IOC_DIRSHIFT) | ((size as u32) << IOC_SIZESHIFT) | io(nr)
}

const UI_DEV_CREATE: u32 = io(1);
const UI_DEV_SETUP: u32 = iow(3, size_of::<UinputSetup>());
const UI_ABS_SETUP: u32 = iow(4, size_of::<UinputAbsSetup>());
const UI_SET_EVBIT: u32 = iow(100, size_of::<libc::c_int>());
const UI_SET_KEYBIT: u32 = iow(101, size_of::<libc::c_int>());
const UI_SET_RELBIT: u32 = iow(102, size_of::<libc::c_int>());
const UI_SET_ABSBIT: u32 = iow(103, size_of::<libc::c_int>());
const UI_SET_PROPBIT: u32 = iow(110, size_of::<libc::c_int>());

#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; UINPUT_MAX_NAME_SIZE],
    ff_effects_max: u32,
}

#[repr(C)]
struct InputAbsinfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

#[repr(C)]
struct UinputAbsSetup {
    code: u16,
    absinfo: InputAbsinfo,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    time: libc::timeval,
    evtype: u16,
    code: u16,
    value: i32,
}

/// Range of an absolute axis, taken from the HID report descriptor.
#[derive(Debug, Clone, Copy)]
pub struct AbsRange {
    pub min: i32,
    pub max: i32,
}

fn ioctl_int(fd: RawFd, request: u32, value: libc::c_int) -> Result<(), Error> {
    if unsafe { libc::ioctl(fd, request as _, value) } < 0 {
        return Err(Error::last_os_error());
    }
    Ok(())
}

fn ioctl_ptr<T>(fd: RawFd, request: u32, arg: &T) -> Result<(), Error> {
    if unsafe { libc::ioctl(fd, request as _, std::ptr::from_ref(arg)) } < 0 {
        return Err(Error::last_os_error());
    }
    Ok(())
}

/// Describes the virtual device to create.
pub struct VirtualDeviceBuilder {
    name: String,
    vendor: u16,
    product: u16,
    keys: Vec<u16>,
    rels: Vec<u16>,
    abs: Vec<(u16, AbsRange)>,
    props: Vec<u16>,
}

impl VirtualDeviceBuilder {
    pub fn new(name: &str, vendor: u16, product: u16) -> VirtualDeviceBuilder {
        VirtualDeviceBuilder {
            name: name.to_string(),
            vendor,
            product,
            keys: vec![],
            rels: vec![],
            abs: vec![],
            props: vec![],
        }
    }

    pub fn key(mut self, code: u16) -> VirtualDeviceBuilder {
        self.keys.push(code);
        self
    }

    pub fn rel(mut self, code: u16) -> VirtualDeviceBuilder {
        self.rels.push(code);
        self
    }

    pub fn abs(mut self, code: u16, range: AbsRange) -> VirtualDeviceBuilder {
        self.abs.push((code, range));
        self
    }

    pub fn prop(mut self, prop: u16) -> VirtualDeviceBuilder {
        self.props.push(prop);
        self
    }

    /// Opens `/dev/uinput`, declares the capabilities and creates the device.
    pub fn build(self) -> Result<VirtualDevice, Error> {
        let file = OpenOptions::new()
            .write(true)
            .open(UINPUT_PATH)
            .map_err(|err| Error::other(format!("Cannot open {UINPUT_PATH}: {err}")))?;
        let fd = file.as_raw_fd();

        ioctl_int(fd, UI_SET_EVBIT, EV_SYN.into())?;

        if !self.keys.is_empty() {
            ioctl_int(fd, UI_SET_EVBIT, EV_KEY.into())?;
            for code in &self.keys {
                ioctl_int(fd, UI_SET_KEYBIT, (*code).into())?;
            }
        }

        if !self.rels.is_empty() {
            ioctl_int(fd, UI_SET_EVBIT, EV_REL.into())?;
            for code in &self.rels {
                ioctl_int(fd, UI_SET_RELBIT, (*code).into())?;
            }
        }

        if !self.abs.is_empty() {
            ioctl_int(fd, UI_SET_EVBIT, EV_ABS.into())?;
            for (code, range) in &self.abs {
                ioctl_int(fd, UI_SET_ABSBIT, (*code).into())?;
                let setup = UinputAbsSetup {
                    code: *code,
                    absinfo: InputAbsinfo {
                        value: 0,
                        minimum: range.min,
                        maximum: range.max,
                        fuzz: 0,
                        flat: 0,
                        resolution: 0,
                    },
                };
                ioctl_ptr(fd, UI_ABS_SETUP, &setup)?;
            }
        }

        for prop in &self.props {
            ioctl_int(fd, UI_SET_PROPBIT, (*prop).into())?;
        }

        let mut setup = UinputSetup {
            id: InputId {
                bustype: BUS_USB,
                vendor: self.vendor,
                product: self.product,
                version: 1,
            },
            name: [0; UINPUT_MAX_NAME_SIZE],
            ff_effects_max: 0,
        };
        let name = self.name.as_bytes();
        let len = name.len().min(UINPUT_MAX_NAME_SIZE - 1);
        setup.name[..len].copy_from_slice(&name[..len]);

        ioctl_ptr(fd, UI_DEV_SETUP, &setup)?;
        ioctl_int(fd, UI_DEV_CREATE, 0)?;

        log::info!("Created virtual device \"{}\"", self.name);

        Ok(VirtualDevice {
            file,
            buffer: Vec::with_capacity(8),
        })
    }
}

/// A virtual input device
pub struct VirtualDevice {
    file: File,
    buffer: Vec<InputEvent>,
}

impl AsRawFd for VirtualDevice {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

impl VirtualDevice {
    /// Sends a batch of `(type, code, value)` events, terminated by a
    /// `SYN_REPORT`.
    pub fn emit(&mut self, events: &[(u16, u16, i32)]) -> Result<(), Error> {
        if events.is_empty() {
            return Ok(());
        }

        self.buffer.clear();
        self.buffer
            .extend(events.iter().map(|(evtype, code, value)| InputEvent {
                time: libc::timeval::default(),
                evtype: *evtype,
                code: *code,
                value: *value,
            }));
        self.buffer.push(InputEvent {
            time: libc::timeval::default(),
            evtype: EV_SYN,
            code: SYN_REPORT,
            value: 0,
        });

        // SAFETY: `InputEvent` is `repr(C)` and contains no padding-sensitive
        // invariant, it is safe to view the slice as bytes.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                self.buffer.as_ptr().cast::<u8>(),
                std::mem::size_of_val(self.buffer.as_slice()),
            )
        };

        self.file.write_all(bytes)
    }
}
