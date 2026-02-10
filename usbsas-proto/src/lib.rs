//! Protobuf generated code
#![allow(clippy::derive_partial_eq_without_eq)]

pub mod analyzer {
    include!(concat!(env!("OUT_DIR"), "/analyzer.rs"));
}

pub mod identifier {
    include!(concat!(env!("OUT_DIR"), "/identifier.rs"));
}

pub mod cmdexec {
    include!(concat!(env!("OUT_DIR"), "/cmdexec.rs"));
}

pub mod common {
    use std::hash::{DefaultHasher, Hash, Hasher};
    include!(concat!(env!("OUT_DIR"), "/common.rs"));

    impl std::fmt::Display for UsbDevice {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(
                f,
                "{} - {} - {} ({}-{})",
                self.manufacturer, self.description, self.serial, self.vendorid, self.productid
            )
        }
    }

    impl std::fmt::Display for FsType {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}", self.as_str_name())
        }
    }

    impl device::Device {
        pub fn is_src(&self) -> bool {
            match self {
                device::Device::Network(net) => net.is_src,
                device::Device::Command(cmd) => cmd.is_src,
                device::Device::Usb(usb) => usb.is_src,
                device::Device::LocalDir(dir) => dir.is_src,
            }
        }
        pub fn is_dst(&self) -> bool {
            match self {
                device::Device::Network(net) => net.is_dst,
                device::Device::Command(cmd) => cmd.is_dst,
                device::Device::Usb(usb) => usb.is_dst,
                device::Device::LocalDir(dir) => dir.is_dst,
            }
        }
        pub fn title(&self) -> &str {
            match self {
                device::Device::Network(net) => &net.title,
                device::Device::Command(cmd) => &cmd.title,
                device::Device::Usb(usb) => &usb.manufacturer,
                device::Device::LocalDir(dir) => &dir.title,
            }
        }
        pub fn description(&self) -> &str {
            match self {
                device::Device::Network(net) => &net.description,
                device::Device::Command(cmd) => &cmd.description,
                device::Device::Usb(usb) => &usb.description,
                device::Device::LocalDir(dir) => &dir.description,
            }
        }
        pub fn id(&self) -> u64 {
            let mut s = DefaultHasher::new();
            self.hash(&mut s);
            s.finish()
        }
    }

    impl From<device::Device> for Device {
        fn from(device: device::Device) -> Self {
            let id = device.id();
            Device {
                device: Some(device),
                id,
            }
        }
    }

    impl From<&usbsas_config::Network> for Network {
        fn from(item: &usbsas_config::Network) -> Self {
            Network {
                url: item.url.clone(),
                krb_service_name: item.krb_service_name.clone(),
                title: item.description.clone(),
                description: item.longdescr.clone(),
                is_src: false,
                is_dst: false,
            }
        }
    }

    impl From<&usbsas_config::LocalDir> for LocalDir {
        fn from(item: &usbsas_config::LocalDir) -> Self {
            LocalDir {
                path: item.path.clone(),
                title: item.description.clone(),
                description: item.longdescr.clone(),
                is_src: false,
                is_dst: false,
            }
        }
    }

    impl From<&usbsas_config::Command> for Command {
        fn from(item: &usbsas_config::Command) -> Self {
            Command {
                bin: item.command_bin.clone(),
                args: item.command_args.clone(),
                title: item.description.clone(),
                description: item.longdescr.clone(),
                is_src: false,
                is_dst: true,
            }
        }
    }

    impl From<&device::Device> for DeviceReport {
        fn from(item: &device::Device) -> Self {
            match &item {
                device::Device::Usb(usb) => DeviceReport {
                    device: Some(device_report::Device::Usb(UsbDeviceReport {
                        vendorid: usb.vendorid,
                        productid: usb.productid,
                        manufacturer: usb.manufacturer.clone(),
                        description: usb.description.clone(),
                        serial: usb.serial.clone(),
                    })),
                },
                device::Device::Network(net) => DeviceReport {
                    device: Some(device_report::Device::Network(NetworkReport {
                        title: net.title.clone(),
                        description: net.description.clone(),
                    })),
                },
                device::Device::Command(cmd) => DeviceReport {
                    device: Some(device_report::Device::Command(CommandReport {
                        title: cmd.title.clone(),
                        description: cmd.description.clone(),
                    })),
                },
                device::Device::LocalDir(dir) => DeviceReport {
                    device: Some(device_report::Device::LocalDir(LocalDirReport {
                        title: dir.title.clone(),
                        description: dir.description.clone(),
                    })),
                },
            }
        }
    }

    impl From<std::fs::FileType> for FileType {
        fn from(ft: std::fs::FileType) -> FileType {
            if ft.is_file() {
                FileType::Regular
            } else if ft.is_dir() {
                FileType::Directory
            } else {
                FileType::Other
            }
        }
    }
}

pub mod downloader {
    include!(concat!(env!("OUT_DIR"), "/downloader.rs"));
}

pub mod files {
    include!(concat!(env!("OUT_DIR"), "/files.rs"));
}

pub mod fs2dev {
    include!(concat!(env!("OUT_DIR"), "/fs2dev.rs"));
}

pub mod scsi {
    include!(concat!(env!("OUT_DIR"), "/scsi.rs"));
}

pub mod uploader {
    include!(concat!(env!("OUT_DIR"), "/uploader.rs"));
}

pub mod usbdev {
    include!(concat!(env!("OUT_DIR"), "/usbdev.rs"));
}

pub mod usbsas {
    include!(concat!(env!("OUT_DIR"), "/usbsas.rs"));
}

pub mod writedst {
    include!(concat!(env!("OUT_DIR"), "/writedst.rs"));
}

pub mod jsonparser {
    include!(concat!(env!("OUT_DIR"), "/jsonparser.rs"));
}
