//! SCSI sense data.

use std::fmt;

/// Sense key, as defined by SPC (SCSI Primary Commands).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenseKey {
    NoSense,
    RecoveredError,
    NotReady,
    MediumError,
    HardwareError,
    IllegalRequest,
    UnitAttention,
    DataProtect,
    BlankCheck,
    VendorSpecific,
    CopyAborted,
    AbortedCommand,
    Reserved,
    VolumeOverflow,
    Miscompare,
    Completed,
}

impl From<u8> for SenseKey {
    fn from(value: u8) -> Self {
        match value & 0x0f {
            0x0 => Self::NoSense,
            0x1 => Self::RecoveredError,
            0x2 => Self::NotReady,
            0x3 => Self::MediumError,
            0x4 => Self::HardwareError,
            0x5 => Self::IllegalRequest,
            0x6 => Self::UnitAttention,
            0x7 => Self::DataProtect,
            0x8 => Self::BlankCheck,
            0x9 => Self::VendorSpecific,
            0xa => Self::CopyAborted,
            0xb => Self::AbortedCommand,
            0xc => Self::Reserved,
            0xd => Self::VolumeOverflow,
            0xe => Self::Miscompare,
            _ => Self::Completed,
        }
    }
}

/// Additional sense code: logical unit not ready.
pub const ASC_LUN_NOT_READY: u8 = 0x04;
/// Additional sense code: invalid command operation code.
pub const ASC_INVALID_OPCODE: u8 = 0x20;
/// Additional sense code: invalid field in CDB.
pub const ASC_INVALID_FIELD_IN_CDB: u8 = 0x24;

/// Decoded sense data, independent of the response format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenseData {
    /// Sense key.
    pub key: SenseKey,
    /// Additional sense code.
    pub asc: u8,
    /// Additional sense code qualifier.
    pub ascq: u8,
}

impl SenseData {
    /// Parse a REQUEST SENSE response, in either fixed (0x70/0x71) or
    /// descriptor (0x72/0x73) format. Returns `None` for an unknown response
    /// code or a truncated buffer.
    pub fn parse(buf: &[u8]) -> Option<Self> {
        match buf.first()? & 0x7f {
            // Fixed format: key in byte 2, ASC/ASCQ in bytes 12/13.
            0x70 | 0x71 => Some(Self {
                key: SenseKey::from(*buf.get(2)?),
                asc: *buf.get(12)?,
                ascq: *buf.get(13)?,
            }),
            // Descriptor format: key in byte 1, ASC/ASCQ in bytes 2/3.
            0x72 | 0x73 => Some(Self {
                key: SenseKey::from(*buf.get(1)?),
                asc: *buf.get(2)?,
                ascq: *buf.get(3)?,
            }),
            _ => None,
        }
    }
}

impl fmt::Display for SenseData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}, ASC {:#04x}, ASCQ {:#04x}",
            self.key, self.asc, self.ascq
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_format() {
        let mut buf = [0u8; 18];
        buf[0] = 0x70;
        buf[2] = 0x02;
        buf[12] = 0x3a;
        buf[13] = 0x01;
        let sense = SenseData::parse(&buf).unwrap();
        assert_eq!(sense.key, SenseKey::NotReady);
        assert_eq!((sense.asc, sense.ascq), (0x3a, 0x01));
    }

    #[test]
    fn fixed_format_with_valid_bit() {
        let mut buf = [0u8; 18];
        buf[0] = 0xf0; // VALID bit set
        buf[2] = 0x06;
        assert_eq!(SenseData::parse(&buf).unwrap().key, SenseKey::UnitAttention);
    }

    #[test]
    fn descriptor_format() {
        let buf = [0x72, 0x06, 0x28, 0x00, 0, 0, 0, 0];
        let sense = SenseData::parse(&buf).unwrap();
        assert_eq!(sense.key, SenseKey::UnitAttention);
        assert_eq!((sense.asc, sense.ascq), (0x28, 0x00));
    }

    #[test]
    fn invalid() {
        assert_eq!(SenseData::parse(&[]), None);
        assert_eq!(SenseData::parse(&[0u8; 18]), None);
        assert_eq!(SenseData::parse(&[0x70, 0, 2]), None);
    }
}
