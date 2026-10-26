use std::{fmt, io};

use crate::sense::SenseData;

/// Phase of a Bulk-Only transaction (or control request) where an error occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Sending the Command Block Wrapper.
    Command,
    /// Receiving data from the device.
    DataIn,
    /// Sending data to the device.
    DataOut,
    /// Receiving the Command Status Wrapper.
    Status,
    /// Mass storage class-specific control request.
    Control,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Command => "command",
            Self::DataIn => "data-in",
            Self::DataOut => "data-out",
            Self::Status => "status",
            Self::Control => "control",
        })
    }
}

/// Errors of this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// USB transfer error
    #[error("usb error during {stage} stage: {source}")]
    Usb {
        /// Stage of the transaction
        stage: Stage,
        /// libusb error
        #[source]
        source: rusb::Error,
    },

    /// Fewer bytes than expected were transferred
    #[error("short {stage} transfer: {actual} of {expected} bytes")]
    ShortTransfer {
        /// Stage of the transaction
        stage: Stage,
        /// Bytes expected
        expected: usize,
        /// Bytes transferred
        actual: usize,
    },

    /// Malformed CSW (bad signature, tag mismatch or unknown status)
    #[error("invalid command status wrapper: {0}")]
    InvalidCsw(String),

    /// The device reported a phase error, it must be reset
    #[error("device reported a phase error")]
    PhaseError,

    #[error("SCSI command {opcode:#04x} failed{}", .sense.map(|s| format!(" ({s})")).unwrap_or_default())]
    /// The SCSI command failed
    CommandFailed {
        /// Opcode of the command
        opcode: u8,
        /// Sense data returned by REQUEST SENSE, if any
        sense: Option<SenseData>,
    },

    /// A parameter doesn't fit in the command
    #[error("{what} out of range: {value}")]
    OutOfRange {
        /// Parameter name
        what: &'static str,
        /// Its value
        value: u64,
    },

    /// Unsupported block size
    #[error("invalid block size: {0}")]
    InvalidBlockSize(u64),

    /// No direct-access or CD/DVD logical unit became ready
    #[error("no ready logical unit found")]
    NoReadyLun,

    /// Device response too short to be parsed
    #[error("Parse error: {0}")]
    ParseError(#[from] std::array::TryFromSliceError),
}

impl Error {
    pub(crate) fn usb(stage: Stage) -> impl FnOnce(rusb::Error) -> Self {
        move |source| Self::Usb { stage, source }
    }
}

/// Map errors to the closest [`io::ErrorKind`].
impl From<Error> for io::Error {
    fn from(err: Error) -> Self {
        let kind = match &err {
            Error::OutOfRange { .. } | Error::InvalidBlockSize(_) => io::ErrorKind::InvalidInput,
            Error::Usb {
                source: rusb::Error::Timeout,
                ..
            } => io::ErrorKind::TimedOut,
            Error::Usb {
                source: rusb::Error::NoDevice,
                ..
            } => io::ErrorKind::NotFound,
            Error::ShortTransfer { .. } => io::ErrorKind::UnexpectedEof,
            Error::InvalidCsw(_) => io::ErrorKind::InvalidData,
            _ => io::ErrorKind::Other,
        };
        io::Error::new(kind, err)
    }
}

/// Result type of this crate.
pub type Result<T> = std::result::Result<T, Error>;
