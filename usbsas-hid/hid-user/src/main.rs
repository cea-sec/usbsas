//! Minimal HID userland driver for X. It only supports left click of mouses and
//! touch screens.

use rusb::{
    request_type, Context, DeviceHandle, Direction, Recipient, RequestType, TransferType,
    UsbContext,
};
use std::{
    collections::HashMap,
    env,
    io::{Error, ErrorKind},
    os::unix::io::AsRawFd,
    ptr::null_mut,
    time::Duration,
};
use x11::{
    xlib::{
        CurrentTime, Display, KeyReleaseMask, XFlush, XOpenDisplay, XRootWindow, XScreenCount,
        XScreenOfDisplay, XSelectInput, XWarpPointer,
    },
    xtest::XTestFakeButtonEvent,
};

enum LibusbClassCode {
    Hid = 0x03,
}

pub struct UsbDevice {
    pub handle: DeviceHandle<Context>,
    pub interface: u8,
    pub ep_in: u8,
    pub ep_in_size: u16,
    _dev_file: std::fs::File,
}

impl UsbDevice {
    pub fn new(
        handle: DeviceHandle<Context>,
        interface: u8,
        ep_in: u8,
        ep_in_size: u16,
        dev_file: std::fs::File,
    ) -> UsbDevice {
        UsbDevice {
            handle,
            interface,
            ep_in,
            ep_in_size,
            _dev_file: dev_file,
        }
    }
}

bitfield::bitfield! {
    struct ItemHdr([u8]);
    impl Debug;
    u8;
    bsize, _: 1, 0;
    btype, _: 3, 2;
    btag, _: 7, 4;

}

fn open_device(busnum: u8, devnum: u8) -> Result<UsbDevice, rusb::Error> {
    rusb::disable_device_discovery()?;
    let libusb_ctx = Context::new()?;

    log::trace!("init dev {} {}", busnum, devnum);

    let device_path = format!("/dev/bus/usb/{:03}/{:03}", busnum, devnum);
    let dev_file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(device_path)
    {
        Ok(file) => file,
        Err(_) => return Err(rusb::Error::NotFound),
    };

    let mut handle = unsafe { libusb_ctx.open_device_with_fd(dev_file.as_raw_fd())? };

    for interface in handle.device().active_config_descriptor()?.interfaces() {
        for desc in interface.descriptors() {
            if desc.class_code() == LibusbClassCode::Hid as u8
            /*
            && (desc.sub_class_code() == 0x01)
            && desc.protocol_code() == 0x2 // Mouse*/
            {
                for endp in desc.endpoint_descriptors() {
                    if endp.transfer_type() == TransferType::Interrupt
                        && endp.direction() == Direction::In
                    {
                        handle.set_auto_detach_kernel_driver(true)?;
                        handle.claim_interface(interface.number())?;
                        log::debug!(
                            "Found interface: {} endpoint {:x}",
                            interface.number(),
                            endp.address()
                        );

                        let result = UsbDevice::new(
                            handle,
                            interface.number(),
                            endp.address(),
                            endp.max_packet_size(),
                            dev_file,
                        );
                        return Ok(result);
                    }
                }
            }
        }
    }
    Err(rusb::Error::NotFound)
}

fn get_item_value_u32(size: u8, buffer: &mut Vec<u8>) -> Result<u32, Error> {
    match size {
        1 => match buffer.pop() {
            Some(value) => {
                let value: u8 = value;
                Ok(value as u32)
            }

            None => Err(Error::new(ErrorKind::Other, "Buffer too short")),
        },
        2 => match (buffer.pop(), buffer.pop()) {
            (Some(value_l), Some(value_h)) => {
                let value: u16 = ((value_h as u16) << 8) | (value_l as u16);
                let value: u16 = value;
                Ok(value as u32)
            }

            _ => Err(Error::new(ErrorKind::Other, "Buffer too short")),
        },
        3 => match (buffer.pop(), buffer.pop(), buffer.pop(), buffer.pop()) {
            (Some(value_b0), Some(value_b1), Some(value_b2), Some(value_b3)) => {
                let value: u32 = ((value_b3 as u32) << 24)
                    | ((value_b2 as u32) << 16)
                    | ((value_b1 as u32) << 8)
                    | (value_b0 as u32);
                let value: u32 = value;
                Ok(value)
            }

            _ => Err(Error::new(ErrorKind::Other, "Buffer too short")),
        },
        _ => {
            todo!("bsize {:?}", size);
        }
    }
}

fn get_item_value_i32(size: u8, buffer: &mut Vec<u8>) -> Result<i32, Error> {
    match size {
        1 => match buffer.pop() {
            Some(value) => {
                let value: i8 = value as i8;
                Ok(value as i32)
            }

            None => Err(Error::new(ErrorKind::Other, "Buffer too short")),
        },
        2 => match (buffer.pop(), buffer.pop()) {
            (Some(value_l), Some(value_h)) => {
                let value: u16 = ((value_h as u16) << 8) | (value_l as u16);
                let value: i16 = value as i16;
                Ok(value as i32)
            }

            _ => Err(Error::new(ErrorKind::Other, "Buffer too short")),
        },
        3 => match (buffer.pop(), buffer.pop(), buffer.pop(), buffer.pop()) {
            (Some(value_b0), Some(value_b1), Some(value_b2), Some(value_b3)) => {
                let value: u32 = ((value_b3 as u32) << 24)
                    | ((value_b2 as u32) << 16)
                    | ((value_b1 as u32) << 8)
                    | (value_b0 as u32);
                let value: i32 = value as i32;
                Ok(value)
            }
            _ => Err(Error::new(ErrorKind::Other, "Buffer too short")),
        },
        _ => {
            todo!("bsize {:?}", size);
        }
    }
}

fn gen_space(indent: usize) -> String {
    "    ".repeat(indent)
}

fn parse_items_attr(value: u32) -> (CoordinateState,) {
    let coordstate = if value & 0b0000_0100 == 0 {
        CoordinateState::Abs
    } else {
        CoordinateState::Rel
    };
    (coordstate,)
}

fn parse_items_attr_to_str(value: u32) -> String {
    let mut specs = vec![];

    if value & 0b0000_0001 == 0 {
        specs.push("Data")
    } else {
        specs.push("Cst")
    }

    if value & 0b0000_0010 == 0 {
        specs.push("Array")
    } else {
        specs.push("Var")
    }

    if value & 0b0000_0100 == 0 {
        specs.push("Abs")
    } else {
        specs.push("Rel")
    }
    /*
    if value & 0b0000_1000 == 0 {
        specs.push("NoWrap")
    } else {
        specs.push("Wrap")
    }

    if value & 0b0001_0000 == 0 {
        specs.push("Lin")
    } else {
        specs.push("NonLin")
    }
     */
    specs.join(",")
}

#[derive(Debug, Clone, std::cmp::PartialEq)]
enum HidUsageGenericDesktop {
    Pointer,
    Mouse,
    Reserved,
    Joystick,
    Game,
    Keyboard,
    Keypad,
    MultiAxisController,
    TabletPCSystemControls,
    X,
    Y,
    Z,
    Rx,
    Ry,
    Rz,
    Slider,
    Dial,
    Wheel,
    HatSwitch,
    CountedBuffer,
    ByteCount,
    MotionWakeup,
    Start,
    Select,
    Vx,
    Vy,
    Vz,

    Unknown(u8),
}

impl HidUsageGenericDesktop {
    fn from_value(value: u8) -> HidUsageGenericDesktop {
        match value {
            0x01 => HidUsageGenericDesktop::Pointer,
            0x02 => HidUsageGenericDesktop::Mouse,
            0x03 => HidUsageGenericDesktop::Reserved,
            0x04 => HidUsageGenericDesktop::Joystick,
            0x05 => HidUsageGenericDesktop::Game,
            0x06 => HidUsageGenericDesktop::Keyboard,
            0x07 => HidUsageGenericDesktop::Keypad,
            0x08 => HidUsageGenericDesktop::MultiAxisController,
            0x09 => HidUsageGenericDesktop::TabletPCSystemControls,
            0x30 => HidUsageGenericDesktop::X,
            0x31 => HidUsageGenericDesktop::Y,
            0x32 => HidUsageGenericDesktop::Z,
            0x33 => HidUsageGenericDesktop::Rx,
            0x34 => HidUsageGenericDesktop::Ry,
            0x35 => HidUsageGenericDesktop::Rz,
            0x36 => HidUsageGenericDesktop::Slider,
            0x37 => HidUsageGenericDesktop::Dial,
            0x38 => HidUsageGenericDesktop::Wheel,
            0x39 => HidUsageGenericDesktop::HatSwitch,
            0x3A => HidUsageGenericDesktop::CountedBuffer,
            0x3B => HidUsageGenericDesktop::ByteCount,
            0x3C => HidUsageGenericDesktop::MotionWakeup,
            0x3D => HidUsageGenericDesktop::Start,
            0x3E => HidUsageGenericDesktop::Select,
            0x40 => HidUsageGenericDesktop::Vx,
            0x41 => HidUsageGenericDesktop::Vy,
            0x42 => HidUsageGenericDesktop::Vz,
            value => HidUsageGenericDesktop::Unknown(value),
        }
    }
}

#[derive(Debug, Clone, std::cmp::PartialEq)]
enum HidUsageDigitizer {
    Digitizer,
    Pen,
    LightPen,
    TouchScreen,
    TouchPad,
    WhiteBoard,
    CoordinateMeasuringMachine,
    ThreeDDigitizer,
    StereoPlotter,
    ArticulatedArm,
    Armature,
    MultiplePointDigitizer,
    FreeSpaceWand,
    Stylus,
    Puck,
    Finger,
    TipPressure,
    BarrelPressure,
    InRange,
    Touch,
    Untouch,
    Tap,
    Quality,
    DataValid,
    TransducerIndex,
    TabletFunctionKeys,
    ProgramChangeKeys,
    BatteryStrength,
    Invert,
    XTilt,
    YTilt,
    Azimuth,
    Altitude,
    Twist,
    TipSwitch,
    SecondaryTipSwitch,
    BarrelSwitch,
    Eraser,
    TabletPick,

    ContactIdentifier,

    Unknown(u8),
}

impl HidUsageDigitizer {
    fn from_value(value: u8) -> HidUsageDigitizer {
        match value {
            0x01 => HidUsageDigitizer::Digitizer,
            0x02 => HidUsageDigitizer::Pen,
            0x03 => HidUsageDigitizer::LightPen,
            0x04 => HidUsageDigitizer::TouchScreen,
            0x05 => HidUsageDigitizer::TouchPad,
            0x06 => HidUsageDigitizer::WhiteBoard,
            0x07 => HidUsageDigitizer::CoordinateMeasuringMachine,
            0x08 => HidUsageDigitizer::ThreeDDigitizer,
            0x09 => HidUsageDigitizer::StereoPlotter,
            0x0A => HidUsageDigitizer::ArticulatedArm,
            0x0B => HidUsageDigitizer::Armature,
            0x0C => HidUsageDigitizer::MultiplePointDigitizer,
            0x0D => HidUsageDigitizer::FreeSpaceWand,
            0x20 => HidUsageDigitizer::Stylus,
            0x21 => HidUsageDigitizer::Puck,
            0x22 => HidUsageDigitizer::Finger,
            0x30 => HidUsageDigitizer::TipPressure,
            0x31 => HidUsageDigitizer::BarrelPressure,
            0x32 => HidUsageDigitizer::InRange,
            0x33 => HidUsageDigitizer::Touch,
            0x34 => HidUsageDigitizer::Untouch,
            0x35 => HidUsageDigitizer::Tap,
            0x36 => HidUsageDigitizer::Quality,
            0x37 => HidUsageDigitizer::DataValid,
            0x38 => HidUsageDigitizer::TransducerIndex,
            0x39 => HidUsageDigitizer::TabletFunctionKeys,
            0x3A => HidUsageDigitizer::ProgramChangeKeys,
            0x3B => HidUsageDigitizer::BatteryStrength,
            0x3C => HidUsageDigitizer::Invert,
            0x3D => HidUsageDigitizer::XTilt,
            0x3E => HidUsageDigitizer::YTilt,
            0x3F => HidUsageDigitizer::Azimuth,
            0x40 => HidUsageDigitizer::Altitude,
            0x41 => HidUsageDigitizer::Twist,
            0x42 => HidUsageDigitizer::TipSwitch,
            0x43 => HidUsageDigitizer::SecondaryTipSwitch,
            0x44 => HidUsageDigitizer::BarrelSwitch,
            0x45 => HidUsageDigitizer::Eraser,
            0x46 => HidUsageDigitizer::TabletPick,
            0x51 => HidUsageDigitizer::ContactIdentifier,
            value => HidUsageDigitizer::Unknown(value),
        }
    }
}

#[derive(Debug, Clone, std::cmp::PartialEq)]
enum HidUsage {
    GenericDesktop(HidUsageGenericDesktop),
    Consumer(u8),
    Digitizer(HidUsageDigitizer),
    Unknown(u16),
}

#[derive(Debug, Clone)]
enum HidUsagePage {
    GenericDesktopControls,
    SimulationControls,
    VRControls,
    SportControls,
    GameControls,
    GenericDeviceControls,
    KeyboardKeypad,
    LEDs,
    Button,
    Ordinal,
    Telephony,
    Consumer,
    Digitizer,
    Reserved,
    PIDPage,
    Unicode,

    Vendor,

    Unknown(u16),
}

impl HidUsagePage {
    fn from_value(value: u16) -> HidUsagePage {
        match value {
            0x01 => HidUsagePage::GenericDesktopControls,
            0x02 => HidUsagePage::SimulationControls,
            0x03 => HidUsagePage::VRControls,
            0x04 => HidUsagePage::SportControls,
            0x05 => HidUsagePage::GameControls,
            0x06 => HidUsagePage::GenericDeviceControls,
            0x07 => HidUsagePage::KeyboardKeypad,
            0x08 => HidUsagePage::LEDs,
            0x09 => HidUsagePage::Button,
            0x0A => HidUsagePage::Ordinal,
            0x0B => HidUsagePage::Telephony,
            0x0C => HidUsagePage::Consumer,
            0x0D => HidUsagePage::Digitizer,
            0x0E => HidUsagePage::Reserved,
            0x0F => HidUsagePage::PIDPage,
            0x10 => HidUsagePage::Unicode,
            0xFF00 => HidUsagePage::Vendor,
            value => HidUsagePage::Unknown(value),
        }
    }
}

#[derive(Debug, Clone)]
enum CoordinateState {
    Abs,
    Rel,
}

#[derive(Debug, Clone, std::cmp::PartialEq)]
enum HidItemType {
    Input,
    Feature,
}

#[derive(Debug, Clone)]
struct HidItem {
    offset: usize,
    usage_page: HidUsagePage,
    usage: Vec<HidUsage>,
    logical_min: i32,
    logical_max: i32,
    coordinatestate: CoordinateState,
    count: u8,
    size: u8,
    r#type: HidItemType,
}

impl HidItem {
    fn get_value(&self, index: usize, buffer: &[u8]) -> Result<i32, Error> {
        if self.logical_min < 0 {
            match self.size {
                size @ 1..=8 => get_i8(
                    buffer,
                    ((self.offset + index * self.size as usize) / 8) as u32,
                    ((self.offset + index * self.size as usize) % 8) as u32,
                    size.into(),
                )
                .map(|value| value as i32),
                size @ 9..=16 => get_i16(
                    buffer,
                    ((self.offset + index * self.size as usize) / 8) as u32,
                    ((self.offset + index * self.size as usize) % 8) as u32,
                    size.into(),
                )
                .map(|value| value as i32),
                _ => Err(Error::new(
                    ErrorKind::Other,
                    format!("Unsupported size for {self:?}"),
                )),
            }
        } else {
            match self.size {
                size @ 1..=8 => get_u8(
                    buffer,
                    ((self.offset + index * self.size as usize) / 8) as u32,
                    ((self.offset + index * self.size as usize) % 8) as u32,
                    size.into(),
                )
                .map(|value| value as i32),
                size @ 9..=16 => get_u16(
                    buffer,
                    ((self.offset + index * self.size as usize) / 8) as u32,
                    ((self.offset + index * self.size as usize) % 8) as u32,
                    size.into(),
                )
                .map(|value| value as i32),
                _ => Err(Error::new(
                    ErrorKind::Other,
                    format!("Unsupported size for {self:?}"),
                )),
            }
        }
    }
}

fn parse_report(mut buffer: Vec<u8>) -> Result<HashMap<u32, (Vec<HidItem>, usize)>, Error> {
    let mut total_report_size: usize = 0;
    let mut local_report_size: Option<usize> = None;
    let mut local_report_count: Option<usize> = None;
    let mut logical_min: Option<i32> = None;
    let mut logical_max: Option<i32> = None;
    let mut indent = 0;
    let mut items = vec![];
    let mut usages = vec![];
    let mut usage_page = None;

    let mut reports = HashMap::new();
    let mut report_id = 0;

    while let Some(value) = buffer.pop() {
        let item = ItemHdr([value]);
        //println!("Value {:#?}", item);
        match item.btype() {
            0 => {
                // Main
                match item.btag() {
                    0b1000 => {
                        // INPUT
                        let value = get_item_value_u32(item.bsize(), &mut buffer)?;
                        let specs = parse_items_attr_to_str(value);
                        let (coordinatestate,) = parse_items_attr(value);
                        log::debug!("{}INPUT({}) ({})", gen_space(indent), value, specs);
                        log::debug!("");
                        match (local_report_count, local_report_size) {
                            (Some(report_count), Some(report_size)) => {
                                if report_count > MAX_REPORT_COUNT {
                                    return Err(Error::new(
                                        ErrorKind::Other,
                                        format!("Strange report count {report_count:?}"),
                                    ));
                                }
                                if report_size > 32 {
                                    return Err(Error::new(
                                        ErrorKind::Other,
                                        format!("Strange report size {report_size:?}"),
                                    ));
                                }

                                //assert!(report_count == usages.len());
                                let item = HidItem {
                                    offset: total_report_size,
                                    usage_page: usage_page.clone().take().ok_or_else(|| {
                                        Error::new(ErrorKind::Other, "No usage page")
                                    })?,
                                    usage: usages.clone(),
                                    logical_min: logical_min.clone().take().ok_or_else(|| {
                                        Error::new(ErrorKind::Other, "No logical min")
                                    })?,
                                    logical_max: logical_max.clone().take().ok_or_else(|| {
                                        Error::new(ErrorKind::Other, "No logical max")
                                    })?,
                                    coordinatestate,
                                    count: report_count as u8,
                                    size: report_size as u8,
                                    r#type: HidItemType::Input,
                                };
                                if report_count != 0 {
                                    items.push(item);
                                }
                                total_report_size += report_size * report_count;
                                usages.clear();
                            }
                            report => {
                                return Err(Error::new(
                                    ErrorKind::Other,
                                    format!("Report size or Report count not set {report:?}"),
                                ));
                            }
                        }
                    }

                    0b1001 => {
                        // OUTPUT
                        let value = get_item_value_u32(item.bsize(), &mut buffer)?;
                        log::debug!("{}OUTPUT({})", gen_space(indent), value);
                    }

                    0b1010 => {
                        // USAGE
                        let value = get_item_value_u32(item.bsize(), &mut buffer)?;
                        let description = match value {
                            0x00 => "Physical".to_string(),
                            0x01 => "Application".to_string(),
                            0x02 => "Logical".to_string(),
                            0x03 => "Report".to_string(),
                            0x04 => "Named Array".to_string(),
                            0x05 => "Usage Switch".to_string(),
                            0x06 => "Usage Modified".to_string(),
                            value => format!("Unknown: {value}"),
                        };

                        log::debug!("{}Collection({})", gen_space(indent), description);
                        indent += 1;
                        usages.clear();
                    }

                    0b1011 => {
                        // FEATURE
                        let value = get_item_value_u32(item.bsize(), &mut buffer)?;
                        let specs = parse_items_attr_to_str(value);
                        let (coordinatestate,) = parse_items_attr(value);
                        log::debug!("{}Feature({:?}) ({})", gen_space(indent), value, specs);
                        log::debug!("{}", gen_space(indent));

                        match (local_report_count, local_report_size) {
                            (Some(report_count), Some(report_size)) => {
                                if report_count > MAX_REPORT_COUNT {
                                    return Err(Error::new(
                                        ErrorKind::Other,
                                        format!("Strange report count {report_count:?}"),
                                    ));
                                }
                                if report_size > 32 {
                                    return Err(Error::new(
                                        ErrorKind::Other,
                                        format!("Strange report size {report_size:?}"),
                                    ));
                                }

                                //assert!(report_count == usages.len());
                                let item = HidItem {
                                    offset: total_report_size,
                                    usage_page: usage_page.clone().take().ok_or_else(|| {
                                        Error::new(ErrorKind::Other, "No usage page")
                                    })?,
                                    usage: usages.clone(),
                                    logical_min: logical_min.clone().take().ok_or_else(|| {
                                        Error::new(ErrorKind::Other, "No logical min")
                                    })?,
                                    logical_max: logical_max.clone().take().ok_or_else(|| {
                                        Error::new(ErrorKind::Other, "No logical max")
                                    })?,
                                    coordinatestate,
                                    count: report_count as u8,
                                    size: report_size as u8,
                                    r#type: HidItemType::Feature,
                                };
                                if report_count != 0 {
                                    items.push(item);
                                }
                                total_report_size += report_size * report_count;
                                usages.clear();
                            }
                            report => {
                                return Err(Error::new(
                                    ErrorKind::Other,
                                    format!("Report size or Report count not set {report:?}"),
                                ));
                            }
                        }
                    }

                    0b1100 => {
                        // END_COLLECTION
                        if item.bsize() != 0 {
                            return Err(Error::new(
                                ErrorKind::Other,
                                "END_COLLECTION size must be 0",
                            ));
                        };
                        indent -= 1;
                        log::debug!("{}END_COLLECTION()", gen_space(indent));
                        log::debug!("{}", gen_space(indent));
                    }

                    btag => {
                        panic!("Unknown btag {btag:b}");
                    }
                }
            }

            1 => {
                // Global
                match item.btag() {
                    0b0000 => {
                        // USAGE_PAGE
                        let value = get_item_value_u32(item.bsize(), &mut buffer)?;
                        let description = match value {
                            0x1 => "Generic Desktop Controls".to_string(),
                            0x02 => "Simulation Controls".to_string(),
                            0x03 => "VR Controls".to_string(),
                            0x04 => "Sport Controls".to_string(),
                            0x05 => "Game Controls".to_string(),
                            0x06 => "Generic Device Controls".to_string(),
                            0x07 => "Keyboard/Keypad".to_string(),
                            0x08 => "LEDs".to_string(),
                            0x09 => "Button".to_string(),
                            0x0A => "Ordinal".to_string(),
                            0x0B => "Telephony".to_string(),
                            0x0C => "Consumer".to_string(),
                            0x0D => "Digitizer".to_string(),
                            0x0E => "Reserved".to_string(),
                            0x0F => "PID Page".to_string(),
                            0x10 => "Unicode".to_string(),
                            value => format!("Unknown: {value}"),
                        };
                        log::debug!("{}Usage_Page({})", gen_space(indent), description);
                        usage_page = Some(HidUsagePage::from_value(value as u16));
                    }

                    0b0001 => {
                        // LOGICAL_MINIMUM
                        let value = get_item_value_i32(item.bsize(), &mut buffer)?;
                        logical_min = Some(value);
                        log::debug!("{}LOGICAL_MINIMUM({})", gen_space(indent), value);
                    }

                    0b0010 => {
                        // LOGICAL_MAXIMUM
                        let value = get_item_value_i32(item.bsize(), &mut buffer)?;
                        logical_max = Some(value);
                        log::debug!("{}LOGICAL_MAXIMUM({})", gen_space(indent), value);
                    }

                    0b0011 => {
                        // PHYSICAL_MINIMUM
                        let value = get_item_value_i32(item.bsize(), &mut buffer)?;
                        log::debug!("{}PHYSICAL_MINIMUM({})", gen_space(indent), value);
                    }

                    0b0100 => {
                        // PHYSICAL_MAXIMUM
                        let value = get_item_value_i32(item.bsize(), &mut buffer)?;
                        log::debug!("{}PHYSICAL_MAXIMUM({})", gen_space(indent), value);
                    }

                    0b0101 => {
                        // UNIT_EXPONENT
                        let value = get_item_value_i32(item.bsize(), &mut buffer)?;
                        log::debug!("{}UNIT_EXPONENT({:?})", gen_space(indent), value);
                    }

                    0b0110 => {
                        // UNITS
                        let value = get_item_value_u32(item.bsize(), &mut buffer)?;

                        let unit_system = value & 0xF;
                        let unit_length = (value >> 4) & 0xF;
                        let unit_mass = (value >> 8) & 0xF;
                        let unit_time = (value >> 12) & 0xF;
                        let unit_temperature = (value >> 16) & 0xF;
                        let unit_current = (value >> 20) & 0xF;
                        let unit_luminosity = (value >> 24) & 0xF;

                        if unit_length != 0 {
                            log::debug!(
                                "{}UNITS(length: {} {})",
                                gen_space(indent),
                                match unit_length {
                                    0 => "None",
                                    1 => "Centimeter",
                                    2 => "Radians",
                                    3 => "Inch",
                                    4 => "Degrees",
                                    _ => "Unknown",
                                },
                                unit_system,
                            );
                        }

                        if unit_mass != 0 {
                            log::debug!(
                                "{}UNITS(length: {} {})",
                                gen_space(indent),
                                unit_length,
                                unit_mass,
                            );
                        }

                        if unit_time != 0 {
                            log::debug!(
                                "{}UNITS(length: {} {})",
                                gen_space(indent),
                                unit_length,
                                unit_time,
                            );
                        }

                        if unit_temperature != 0 {
                            log::debug!(
                                "{}UNITS(length: {} {})",
                                gen_space(indent),
                                unit_temperature,
                                unit_system,
                            );
                        }

                        if unit_current != 0 {
                            log::debug!(
                                "{}UNITS(length: {} {})",
                                gen_space(indent),
                                unit_current,
                                unit_system,
                            );
                        }

                        if unit_luminosity != 0 {
                            log::debug!(
                                "{}UNITS(length: {} {})",
                                gen_space(indent),
                                unit_luminosity,
                                unit_system,
                            );
                        }
                    }

                    0b0111 => {
                        // REPORT_SIZE
                        let value = get_item_value_u32(item.bsize(), &mut buffer)?;
                        log::debug!("{}REPORT_SIZE({})", gen_space(indent), value);
                        local_report_size = Some(value as usize);
                    }

                    0b1000 => {
                        // REPORT_ID
                        if !items.is_empty() {
                            reports.insert(report_id, (items.clone(), total_report_size));
                        }

                        let value = get_item_value_u32(item.bsize(), &mut buffer)?;
                        log::debug!("{}REPORT_ID({:?})", gen_space(indent), value);
                        report_id = value;
                        items.clear();
                        total_report_size = 8;
                    }

                    0b1001 => {
                        // REPORT_COUNT
                        let value = get_item_value_u32(item.bsize(), &mut buffer)?;
                        log::debug!("{}REPORT_COUNT({})", gen_space(indent), value);
                        local_report_count = Some(value as usize);
                    }

                    btag => {
                        panic!("Unknown btag {btag:b}");
                    }
                }
            }
            2 => {
                // Local
                match item.btag() {
                    0b0000 => {
                        // USAGE
                        let value = get_item_value_u32(item.bsize(), &mut buffer)?;
                        let usage = match &usage_page {
                            Some(HidUsagePage::GenericDesktopControls) => HidUsage::GenericDesktop(
                                HidUsageGenericDesktop::from_value(value as u8),
                            ),
                            Some(HidUsagePage::Consumer) => HidUsage::Consumer(value as u8),
                            Some(HidUsagePage::Digitizer) => {
                                HidUsage::Digitizer(HidUsageDigitizer::from_value(value as u8))
                            }
                            Some(HidUsagePage::Vendor) => HidUsage::Unknown(value as u16),
                            _ => {
                                log::debug!("Unknown usage: {:?}", value);
                                HidUsage::Unknown(value as u16)
                            }
                        };
                        log::debug!("{}Usage({:?})", gen_space(indent), usage);
                        usages.push(usage);
                    }

                    0b0001 => {
                        // USAGE_MINIMUM
                        let value = get_item_value_i32(item.bsize(), &mut buffer)?;
                        log::debug!("{}USAGE_MINIMUM({})", gen_space(indent), value);
                    }

                    0b0010 => {
                        // USAGE_MAXIMUM
                        let value = get_item_value_i32(item.bsize(), &mut buffer)?;
                        log::debug!("{}USAGE_MAXIMUM({})", gen_space(indent), value);
                    }

                    btag => {
                        panic!("Unknown btag {btag:b}");
                    }
                }
            }
            _ => {
                panic!("Unknown btype");
            }
        }
    }

    if !items.is_empty() {
        reports.insert(report_id, (items, total_report_size));
        if total_report_size % 8 != 0 {
            return Err(Error::new(ErrorKind::Other, "Size report is not % 8"));
        }
    }

    log::debug!("Items: ");
    for (report_id, report) in reports.iter() {
        log::debug!("Report {} size: {}", report_id, report.1 / 8);
        for item in report.0.iter() {
            log::debug!("    {:?}", item);
        }
    }
    Ok(reports)
}

fn get_u8(
    buffer: &[u8],
    mut byte_offset: u32,
    mut bit_offset: u32,
    length: u32,
) -> Result<u8, Error> {
    if length == 0 {
        return Err(Error::new(ErrorKind::Other, "bad len"));
    }

    if length > 8 {
        return Err(Error::new(ErrorKind::Other, "out of range"));
    }

    // Ensure that we stay within the vector
    if (buffer.len() as u32 * 8) < byte_offset * 8 + bit_offset + length {
        return Err(Error::new(ErrorKind::Other, "out of range"));
    }

    byte_offset += bit_offset / 8;
    bit_offset %= 8;

    if bit_offset + length <= 8 {
        let mut copy: u8 = buffer[byte_offset as usize];
        // Clear the high bits
        copy <<= 8 - (bit_offset + length);
        copy >>= 8 - length;
        Ok(copy)
    } else {
        // The range of bits spans over 2 bytes (not more)
        // Copy the first byte
        let copy2: u8 = buffer[byte_offset as usize];

        // Now copy the second bytes
        let copy1: u8 = buffer[byte_offset as usize + 1];

        // Copy that into a bigger variable type
        let mut copy1_as_u16: u16 = copy1 as u16;

        // Shift 8 bits to the left, since these are the first 2 of 3 bytes
        copy1_as_u16 <<= 8;

        // Logical OR these two to get the original 2 bytes
        let mut result = copy1_as_u16 | (copy2 as u16);

        // From now on, process like the normal case above
        result <<= 16 - (bit_offset + length);
        result >>= 16 - length;
        Ok(result as u8)
    }
}

fn get_i8(
    buffer: &[u8],
    mut byte_offset: u32,
    mut bit_offset: u32,
    length: u32,
) -> Result<i8, Error> {
    if length == 0 {
        return Err(Error::new(ErrorKind::Other, "bad len"));
    }

    if length > 8 {
        return Err(Error::new(ErrorKind::Other, "out of range"));
    }

    // Ensure that we stay within the vector
    if (buffer.len() as u32 * 8) < byte_offset * 8 + bit_offset + length {
        return Err(Error::new(ErrorKind::Other, "out of range"));
    }

    byte_offset += bit_offset / 8;
    bit_offset %= 8;

    if bit_offset + length <= 8 {
        let byte = buffer[byte_offset as usize];
        let mut value = byte;
        // Clear the high bits
        value <<= 8 - (bit_offset + length);
        value >>= 8 - length;
        if length < 8 && value & (1 << (length - 1)) != 0 {
            value |= 0xffu8.wrapping_shl(length);
        }
        Ok(value as i8)
    } else {
        // The range of bits spans over 2 bytes (not more)
        let byte2: u8 = buffer[byte_offset as usize];
        let byte1: u8 = buffer[byte_offset as usize + 1];

        let mut value: u16 = ((byte1 as u16) << 8) | (byte2 as u16);

        value <<= 16 - (bit_offset + length);
        value >>= 16 - length;
        if length < 8 && value & (1 << (length - 1)) != 0 {
            value |= 0xffffu16.wrapping_shl(length);
        }
        Ok(value as i8)
    }
}

fn get_i16(
    buffer: &[u8],
    mut byte_offset: u32,
    mut bit_offset: u32,
    length: u32,
) -> Result<i16, Error> {
    if length == 0 {
        return Err(Error::new(ErrorKind::Other, "bad len"));
    };

    if length > 16 {
        return Err(Error::new(ErrorKind::Other, "out of range"));
    }

    // Ensure that we stay within the vector
    if (buffer.len() as u32 * 8) < byte_offset * 8 + bit_offset + length {
        return Err(Error::new(ErrorKind::Other, "out of range"));
    }

    byte_offset += bit_offset / 8;
    bit_offset %= 8;

    if bit_offset + length <= 8 {
        let byte = buffer[byte_offset as usize];
        let mut value = byte as u16;

        // Clear the high bits
        value <<= 16 - (bit_offset + length);
        value >>= 16 - length;
        if length < 16 && value & (1 << (length - 1)) != 0 {
            value |= 0xffffu16.wrapping_shl(length);
        }
        Ok(value as i16)
    } else if bit_offset + length <= 16 {
        let byte2 = buffer[byte_offset as usize] as u16;
        let byte1 = buffer[byte_offset as usize + 1] as u16;

        let mut value = (byte1 << 8) | byte2;

        // Clear the high bits
        value <<= 16 - (bit_offset + length);
        value >>= 16 - length;
        if length < 16 && value & (1 << (length - 1)) != 0 {
            value |= 0xffffu16.wrapping_shl(length);
        }
        Ok(value as i16)
    } else {
        // The range of bits spans over 3 bytes (not more)
        let byte3 = buffer[byte_offset as usize];
        let byte2 = buffer[byte_offset as usize + 1];
        let byte1 = buffer[byte_offset as usize + 2];

        let mut value = ((byte1 as u32) << 16) | ((byte2 as u32) << 8) | (byte3 as u32);

        value <<= 32 - (bit_offset + length);
        value >>= 32 - length;
        if length < 16 && value & (1 << (length - 1)) != 0 {
            value |= 0xffffffffu32.wrapping_shl(length);
        }
        Ok(value as i16)
    }
}

fn get_u16(
    buffer: &[u8],
    mut byte_offset: u32,
    mut bit_offset: u32,
    length: u32,
) -> Result<u16, Error> {
    if length == 0 {
        return Err(Error::new(ErrorKind::Other, "bad len"));
    };

    if length > 16 {
        return Err(Error::new(ErrorKind::Other, "out of range"));
    }
    // Ensure that we stay within the vector
    if (buffer.len() as u32 * 8) < byte_offset * 8 + bit_offset + length {
        return Err(Error::new(ErrorKind::Other, "out of range"));
    }

    byte_offset += bit_offset / 8;
    bit_offset -= (bit_offset / 8) * 8;

    if bit_offset + length <= 8 {
        let byte = buffer[byte_offset as usize];
        let mut value = byte as u16;

        // Clear the high bits
        value <<= 16 - (bit_offset + length);
        value >>= 16 - length;
        Ok(value)
    } else if bit_offset + length <= 16 {
        let byte2 = buffer[byte_offset as usize] as u16;
        let byte1 = buffer[byte_offset as usize + 1] as u16;

        let mut value = (byte1 << 8) | byte2;

        // Clear the high bits
        value <<= 16 - (bit_offset + length);
        value >>= 16 - length;
        Ok(value)
    } else {
        // The range of bits spans over 3 bytes (not more)
        let byte3 = buffer[byte_offset as usize];
        let byte2 = buffer[byte_offset as usize + 1];
        let byte1 = buffer[byte_offset as usize + 2];

        let mut value = ((byte1 as u32) << 16) | ((byte2 as u32) << 8) | (byte3 as u32);

        value <<= 32 - (bit_offset + length);
        value >>= 32 - length;
        Ok(value as u16)
    }
}

struct ScreenInfo {
    display: *mut Display,
    rootwindow: u64,
    width: i32,
    height: i32,
}

fn get_screen_display() -> Result<ScreenInfo, Error> {
    /* X11 */
    let display = unsafe { XOpenDisplay(null_mut()) };
    if display.is_null() {
        return Err(Error::new(
            ErrorKind::Other,
            "Error in XOpenDisplay".to_string(),
        ));
    }
    log::debug!("Display: {:?}", display);
    let rootwindow = unsafe { XRootWindow(display, 0) };
    log::debug!("Root windows: {:?}", rootwindow);
    unsafe { XSelectInput(display, rootwindow, KeyReleaseMask) };

    /* Get screen size */
    let count_screens = unsafe { XScreenCount(display) };
    assert!(count_screens > 0);

    let screen = unsafe { XScreenOfDisplay(display, 0) };
    let width = unsafe { (*screen).width };
    let height = unsafe { (*screen).height };

    Ok(ScreenInfo {
        display,
        rootwindow,
        width,
        height,
    })
}

const MAX_HID_DESCRIPTOR_LENGTH: usize = 1000;
const MAX_REPORT_COUNT: usize = 2048;

fn get_hid_descriptor(device: &UsbDevice) -> Result<Vec<u8>, Error> {
    let mut buffer: Vec<u8> = vec![0; MAX_HID_DESCRIPTOR_LENGTH];

    let len = device
        .handle
        .read_control(
            request_type(Direction::In, RequestType::Standard, Recipient::Interface),
            0x6,    // Request descriptor
            0x2200, // HID Report
            0,
            &mut buffer,
            Duration::from_secs(5),
        )
        .map_err(|err| {
            Error::new(
                ErrorKind::Other,
                format!("Error in get HID descriptor {err:?}"),
            )
        })?;
    if len >= buffer.len() {
        return Err(Error::new(ErrorKind::Other, "Response too big"));
    }
    buffer.truncate(len);
    log::debug!("{}, {:?}", len, buffer);

    buffer.reverse();
    Ok(buffer)
}

fn get_associated_report<'a>(
    buffer: &[u8],
    reports: &'a HashMap<u32, (Vec<HidItem>, usize)>,
) -> Result<&'a (Vec<HidItem>, usize), Error> {
    let index = if reports.len() == 1 {
        0u32
    } else {
        *buffer
            .first()
            .ok_or_else(|| Error::new(ErrorKind::Other, "Empty buffer"))? as u32
    };

    reports
        .get(&index)
        .ok_or_else(|| Error::new(ErrorKind::Other, "No default report"))
}

struct DisplayContext {
    screen: ScreenInfo,
    cursor_x: i32,
    cursor_y: i32,
    surface_touched: bool,
    change_surface: bool,
    button_num: usize,
    finger_touch: bool,
}

impl DisplayContext {
    fn new(screen: ScreenInfo) -> DisplayContext {
        let cursor_x = screen.width / 2;
        let cursor_y = screen.height / 2;
        DisplayContext {
            screen,
            cursor_x,
            cursor_y,
            surface_touched: false,
            change_surface: false,
            button_num: 0,
            finger_touch: false,
        }
    }

    fn updt_cursor(&self) {
        /* Move cursor */
        unsafe {
            XWarpPointer(
                self.screen.display,
                0,
                self.screen.rootwindow,
                0,
                0,
                0,
                0,
                self.cursor_x,
                self.cursor_y,
            )
        };
        unsafe { XFlush(self.screen.display) };
    }
}

fn parse_generic_desktop_control(
    context: &mut DisplayContext,
    item: &HidItem,
    buffer: &[u8],
) -> Result<(), Error> {
    for index in 0..item.count as usize {
        // TODO check min max
        let value = item.get_value(index, buffer)?;
        let usage = &item.usage[index];

        match usage {
            HidUsage::GenericDesktop(HidUsageGenericDesktop::X) => {
                log::trace!("value x {}", value);
                match item.coordinatestate {
                    CoordinateState::Abs => {
                        if context.finger_touch {
                            context.cursor_x = ((value) * context.screen.width) / item.logical_max;
                        }
                    }
                    CoordinateState::Rel => {
                        let logical_center_x = (item.logical_max + item.logical_min) / 2;
                        context.cursor_x += value - logical_center_x;
                    }
                }
            }
            HidUsage::GenericDesktop(HidUsageGenericDesktop::Y) => {
                log::trace!("value y {}", value);
                match item.coordinatestate {
                    CoordinateState::Abs => {
                        if context.finger_touch {
                            context.cursor_y = (value * context.screen.height) / item.logical_max;
                        }
                    }
                    CoordinateState::Rel => {
                        let logical_center_y = (item.logical_max + item.logical_min) / 2;
                        context.cursor_y += value - logical_center_y;
                    }
                }
            }
            HidUsage::GenericDesktop(HidUsageGenericDesktop::Wheel) => {
                // TODO
            }
            HidUsage::GenericDesktop(HidUsageGenericDesktop::Rz) => {
                // TODO
            }
            HidUsage::GenericDesktop(HidUsageGenericDesktop::Slider) => {
                // TODO
            }
            usage => {
                log::debug!("Unsupported item {:?}", usage);
            }
        };
    }
    Ok(())
}

fn parse_digitizer(
    context: &mut DisplayContext,
    item: &HidItem,
    buffer: &[u8],
) -> Result<(), Error> {
    log::trace!("item {:?}", item);
    if item.count as usize != item.usage.len() {
        return Ok(());
    }
    for index in 0..item.count as usize {
        let usage = &item.usage[index];
        log::trace!("item usage {:?}", usage);
        match usage {
            HidUsage::Digitizer(HidUsageDigitizer::TipSwitch) => {
                if item.size != 1 {
                    continue;
                }
                let button_value = item.get_value(index, buffer)?;
                log::trace!("button {} {}", index, button_value);
                if button_value != 0 {
                    context.finger_touch = true;
                    context.surface_touched = true;
                } else {
                    context.finger_touch = false;
                }
                context.change_surface = true;
            }
            usage => {
                log::debug!("Unsupported item {:?}", usage);
            }
        }
    }
    Ok(())
}

fn parse_button(context: &mut DisplayContext, item: &HidItem, buffer: &[u8]) -> Result<(), Error> {
    if item.size != 1 {
        return Ok(());
    }
    for index in 0..item.count as usize {
        let button_value = item.get_value(index, buffer)?;
        if context.button_num == 0 {
            // Only forward left button
            unsafe {
                if context.button_num < 10 {
                    XTestFakeButtonEvent(
                        context.screen.display,
                        context.button_num as u32 + 1,
                        button_value & 1,
                        CurrentTime,
                    );
                }
            };
            unsafe { XFlush(context.screen.display) };
            context.button_num += 1;
        }
    }
    Ok(())
}

fn request_reports(device: &UsbDevice, reports: &HashMap<u32, (Vec<HidItem>, usize)>) {
    for (report_id, (items, size)) in reports {
        let mut buffer = vec![0u8; *size / 8];
        let result = device.handle.read_control(
            request_type(Direction::In, RequestType::Class, Recipient::Interface),
            0x1,                                 // GET_REPORT
            (0x3u16 << 8) | (*report_id as u16), //ReportType=Feature (3)
            device.interface as u16,
            &mut buffer,
            Duration::from_millis(1000),
        );
        log::debug!("Get report id: {:?} {:?}, {:?}", report_id, result, buffer);

        if !items.iter().all(|item| item.r#type == HidItemType::Feature) {
            continue;
        }

        if items.len() == 1 {
            let usages = vec![
                HidUsage::Digitizer(HidUsageDigitizer::Unknown(82)),
                HidUsage::Digitizer(HidUsageDigitizer::Unknown(83)),
            ];
            if items[0]
                .usage
                .iter()
                .zip(usages.iter())
                .all(|(a, b)| a == b)
            {
                let buffer = [0x0, 0x0, 0x0];
                let result = device.handle.write_control(
                    request_type(Direction::Out, RequestType::Class, Recipient::Interface),
                    0x9,                                 // SET_REPORT
                    (0x3u16 << 8) | (*report_id as u16), // ReportType=Feature (3)
                    0,
                    &buffer,
                    Duration::from_millis(1000),
                );
                log::debug!("Set report {:?} {:?}, {:?}", report_id, result, buffer);
            }
        }
    }
}

fn main() -> Result<(), Error> {
    env_logger::Builder::from_default_env()
        .target(env_logger::Target::Stdout)
        .init();

    let busnum = env::var("BUSNUM")
        .map_err(|err| Error::new(ErrorKind::Other, format!("Cannot get BUSNUM env: {err:?}")))?;

    let devnum = env::var("DEVNUM")
        .map_err(|err| Error::new(ErrorKind::Other, format!("Cannot get DEVNUM env: {err:?}")))?;

    let busnum = busnum.parse::<u8>().map_err(|err| {
        Error::new(
            ErrorKind::Other,
            format!("BUSNUM is not an integer: {err:?}"),
        )
    })?;

    let devnum = devnum.parse::<u8>().map_err(|err| {
        Error::new(
            ErrorKind::Other,
            format!("DEVNUM is not an integer: {err:?}"),
        )
    })?;

    assert!(rusb::supports_detach_kernel_driver());

    log::info!("Busnum: {} Devnum: {}", busnum, devnum);

    let device = open_device(busnum, devnum)
        .map_err(|err| Error::new(ErrorKind::Other, format!("Usb device error: {err:?}")))?;

    let buffer = get_hid_descriptor(&device)?;
    let reports = parse_report(buffer)?;
    /* Request reports */
    request_reports(&device, &reports);

    let screen = get_screen_display()?;
    log::debug!("Screen: {}x{}", screen.width, screen.height);

    let mut context = DisplayContext::new(screen);
    context.updt_cursor();

    let mut buffer = vec![0; device.ep_in_size as usize];
    loop {
        match device
            .handle
            .read_interrupt(device.ep_in, &mut buffer, Duration::from_millis(1000))
        {
            Ok(_) => {
                context.surface_touched = false;
                context.button_num = 0;
                context.finger_touch = false;

                log::debug!("Read: {:?}", buffer);
                let report = get_associated_report(&buffer, &reports)?;
                for item in report.0.iter() {
                    match &item.usage_page {
                        HidUsagePage::Button => {
                            parse_button(&mut context, item, &buffer)?;
                        }
                        HidUsagePage::GenericDesktopControls => {
                            parse_generic_desktop_control(&mut context, item, &buffer)?;
                        }
                        HidUsagePage::Consumer => {
                            // TODO
                        }
                        HidUsagePage::Digitizer => {
                            parse_digitizer(&mut context, item, &buffer)?;
                        }
                        value => {
                            log::debug!("Unknown: {:?}", value);
                        }
                    }
                }

                unsafe {
                    XWarpPointer(
                        context.screen.display,
                        0,
                        context.screen.rootwindow,
                        0,
                        0,
                        0,
                        0,
                        context.cursor_x,
                        context.cursor_y,
                    )
                };
                if context.change_surface {
                    let value = i32::from(context.surface_touched);
                    unsafe {
                        XTestFakeButtonEvent(context.screen.display, 1, value, CurrentTime);
                    };
                }
                unsafe { XFlush(context.screen.display) };
            }
            Err(rusb::Error::Timeout) => {
                // skip
            }
            Err(err) => {
                log::error!("Err {:?}, exiting", err);
                break;
            }
        }
    }
    Ok(())
}
