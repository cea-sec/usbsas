//! Protobuf generated code
#![allow(clippy::derive_partial_eq_without_eq)]

pub mod analyzer {
    include!(concat!(env!("OUT_DIR"), "/analyzer.rs"));
}

pub mod identificator {
    include!(concat!(env!("OUT_DIR"), "/identificator.rs"));
}

pub mod cmdexec {
    include!(concat!(env!("OUT_DIR"), "/cmdexec.rs"));
}

pub mod common {
    include!(concat!(env!("OUT_DIR"), "/common.rs"));

    impl std::fmt::Display for Device {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(
                f,
                "{} - {} - {} ({}-{})",
                self.manufacturer, self.description, self.serial, self.vendorid, self.productid
            )
        }
    }
}

pub mod downloader {
    include!(concat!(env!("OUT_DIR"), "/downloader.rs"));
}

pub mod files {
    include!(concat!(env!("OUT_DIR"), "/files.rs"));
}

pub mod filter {
    include!(concat!(env!("OUT_DIR"), "/filter.rs"));
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

pub mod writefs {
    include!(concat!(env!("OUT_DIR"), "/writefs.rs"));
}

pub mod writetar {
    include!(concat!(env!("OUT_DIR"), "/writetar.rs"));
}
