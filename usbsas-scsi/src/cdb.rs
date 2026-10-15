//! SCSI Command Descriptor Blocks.

const TEST_UNIT_READY: u8 = 0x00;
const REQUEST_SENSE: u8 = 0x03;
const INQUIRY: u8 = 0x12;
const READ_CAPACITY_10: u8 = 0x25;
const READ_10: u8 = 0x28;
const WRITE_10: u8 = 0x2a;

/// A command descriptor block, padded to the 16 bytes a CBW can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cdb {
    bytes: [u8; 16],
    len: u8,
}

impl Cdb {
    fn new<const N: usize>(cdb: [u8; N]) -> Self {
        const { assert!(N > 0 && N <= 16) };
        let mut bytes = [0; 16];
        bytes[..N].copy_from_slice(&cdb);
        Self {
            bytes,
            len: N as u8,
        }
    }

    fn rw10(opcode: u8, lba: u32, blocks: u16) -> Self {
        let [l0, l1, l2, l3] = lba.to_be_bytes();
        let [b0, b1] = blocks.to_be_bytes();
        Self::new([opcode, 0, l0, l1, l2, l3, 0, b0, b1, 0])
    }

    pub(crate) fn test_unit_ready() -> Self {
        Self::new([TEST_UNIT_READY, 0, 0, 0, 0, 0])
    }

    pub(crate) fn request_sense(allocation_length: u8) -> Self {
        Self::new([REQUEST_SENSE, 0, 0, 0, allocation_length, 0])
    }

    pub(crate) fn inquiry(allocation_length: u8) -> Self {
        Self::new([INQUIRY, 0, 0, 0, allocation_length, 0])
    }

    pub(crate) fn read_capacity_10() -> Self {
        Self::new([READ_CAPACITY_10, 0, 0, 0, 0, 0, 0, 0, 0, 0])
    }

    pub(crate) fn read_10(lba: u32, blocks: u16) -> Self {
        Self::rw10(READ_10, lba, blocks)
    }

    pub(crate) fn write_10(lba: u32, blocks: u16) -> Self {
        Self::rw10(WRITE_10, lba, blocks)
    }

    pub(crate) fn opcode(&self) -> u8 {
        self.bytes[0]
    }

    pub(crate) fn len(&self) -> u8 {
        self.len
    }

    /// The CDB padded with zeroes to 16 bytes, as laid out in a CBW.
    pub(crate) fn padded(&self) -> &[u8; 16] {
        &self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_byte_commands() {
        let cdb = Cdb::inquiry(36);
        assert_eq!(cdb.len(), 6);
        assert_eq!(&cdb.padded()[..6], &[0x12, 0, 0, 0, 36, 0]);
        assert_eq!(Cdb::request_sense(18).padded()[4], 18);
        assert_eq!(Cdb::test_unit_ready().len(), 6);
    }

    #[test]
    fn read_write_10() {
        let cdb = Cdb::read_10(0x0102_0304, 0x0506);
        assert_eq!(cdb.len(), 10);
        assert_eq!(
            &cdb.padded()[..10],
            &[0x28, 0, 0x01, 0x02, 0x03, 0x04, 0, 0x05, 0x06, 0]
        );
        assert!(cdb.padded()[10..].iter().all(|&b| b == 0));
        assert_eq!(Cdb::write_10(0, 1).opcode(), 0x2a);
        assert_eq!(Cdb::read_capacity_10().len(), 10);
    }
}
