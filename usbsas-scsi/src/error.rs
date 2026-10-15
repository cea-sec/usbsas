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

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("usb error during {stage} stage: {source}")]
    Usb {
        stage: Stage,
        #[source]
        source: rusb::Error,
    },

    #[error("short {stage} transfer: {actual} of {expected} bytes")]
    ShortTransfer {
        stage: Stage,
        expected: usize,
        actual: usize,
    },

    #[error("invalid command status wrapper: {0}")]
    InvalidCsw(String),

    #[error("device reported a phase error")]
    PhaseError,

    #[error("SCSI command {opcode:#04x} failed{}", .sense.map(|s| format!(" ({s})")).unwrap_or_default())]
    CommandFailed {
        opcode: u8,
        sense: Option<SenseData>,
    },

    #[error("{what} out of range: {value}")]
    OutOfRange { what: &'static str, value: u64 },

    #[error("invalid block size: {0}")]
    InvalidBlockSize(u64),

    #[error("no ready logical unit found")]
    NoReadyLun,

    #[error("Parse error: {0}")]
    ParseError(#[from] std::array::TryFromSliceError),
}

impl Error {
    pub(crate) fn usb(stage: Stage) -> impl FnOnce(rusb::Error) -> Self {
        move |source| Self::Usb { stage, source }
    }
}

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

pub type Result<T> = std::result::Result<T, Error>;
