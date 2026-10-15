//! USB Mass Storage Bulk-Only Transport wire formats.

use crate::{
    cdb::Cdb,
    error::{Error, Result},
};

pub(crate) const CBW_LEN: usize = 31;
pub(crate) const CSW_LEN: usize = 13;

const CBW_SIGNATURE: u32 = 0x4342_5355; // "USBC"
const CSW_SIGNATURE: u32 = 0x5342_5355; // "USBS"

const CBW_FLAG_DATA_IN: u8 = 0x80;
const CBW_FLAG_DATA_OUT: u8 = 0x00;

/// Direction of the data stage, from the host's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    In,
    Out,
}

/// Command Block Wrapper.
#[derive(Debug)]
pub(crate) struct Cbw<'a> {
    pub tag: u32,
    pub transfer_length: u32,
    pub direction: Direction,
    pub lun: u8,
    pub cdb: &'a Cdb,
}

impl Cbw<'_> {
    pub(crate) fn to_bytes(&self) -> [u8; CBW_LEN] {
        let mut cbw = [0; CBW_LEN];
        cbw[0..4].copy_from_slice(&CBW_SIGNATURE.to_le_bytes());
        cbw[4..8].copy_from_slice(&self.tag.to_le_bytes());
        cbw[8..12].copy_from_slice(&self.transfer_length.to_le_bytes());
        cbw[12] = match self.direction {
            Direction::In => CBW_FLAG_DATA_IN,
            Direction::Out => CBW_FLAG_DATA_OUT,
        };
        cbw[13] = self.lun & 0x0f;
        cbw[14] = self.cdb.len();
        cbw[15..31].copy_from_slice(self.cdb.padded());
        cbw
    }
}

/// `bCSWStatus` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CswStatus {
    Passed,
    Failed,
    PhaseError,
}

/// Command Status Wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Csw {
    pub residue: u32,
    pub status: CswStatus,
}

impl Csw {
    /// Parse and validate a CSW answering the command `expected_tag`.
    pub(crate) fn parse(buf: &[u8; CSW_LEN], expected_tag: u32) -> Result<Self> {
        let signature = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if signature != CSW_SIGNATURE {
            return Err(Error::InvalidCsw(format!(
                "bad signature {signature:#010x}"
            )));
        }

        let tag = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        if tag != expected_tag {
            return Err(Error::InvalidCsw(format!(
                "tag mismatch: expected {expected_tag:#x}, got {tag:#x}"
            )));
        }

        let status = match buf[12] {
            0 => CswStatus::Passed,
            1 => CswStatus::Failed,
            2 => CswStatus::PhaseError,
            other => return Err(Error::InvalidCsw(format!("unknown status {other:#04x}"))),
        };

        Ok(Self {
            residue: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csw(tag: u32, residue: u32, status: u8) -> [u8; CSW_LEN] {
        let mut buf = [0; CSW_LEN];
        buf[0..4].copy_from_slice(b"USBS");
        buf[4..8].copy_from_slice(&tag.to_le_bytes());
        buf[8..12].copy_from_slice(&residue.to_le_bytes());
        buf[12] = status;
        buf
    }

    #[test]
    fn cbw_layout() {
        let cdb = Cdb::read_10(0x10, 4);
        let cbw = Cbw {
            tag: 0xdead_beef,
            transfer_length: 2048,
            direction: Direction::In,
            lun: 1,
            cdb: &cdb,
        }
        .to_bytes();
        assert_eq!(&cbw[0..4], b"USBC");
        assert_eq!(&cbw[4..8], &[0xef, 0xbe, 0xad, 0xde]);
        assert_eq!(&cbw[8..12], &2048u32.to_le_bytes());
        assert_eq!(cbw[12], 0x80);
        assert_eq!(cbw[13], 1);
        assert_eq!(cbw[14], 10);
        assert_eq!(&cbw[15..31], cdb.padded());
    }

    #[test]
    fn csw_valid() {
        let parsed = Csw::parse(&csw(7, 512, 1), 7).unwrap();
        assert_eq!(parsed.status, CswStatus::Failed);
        assert_eq!(parsed.residue, 512);
        assert_eq!(
            Csw::parse(&csw(7, 0, 2), 7).unwrap().status,
            CswStatus::PhaseError
        );
    }

    #[test]
    fn csw_invalid() {
        assert!(Csw::parse(&csw(7, 0, 0), 8).is_err());
        assert!(Csw::parse(&csw(7, 0, 3), 7).is_err());
        let mut bad = csw(7, 0, 0);
        bad[0] = b'X';
        assert!(Csw::parse(&bad, 7).is_err());
    }
}
