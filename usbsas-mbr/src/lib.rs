//! Master boot record parser

use byteorder::{ByteOrder, LittleEndian};

use std::io;

/// mbr standard magic number
/// # Value
/// ```
/// pub const MBR_SIGNATURE : [u8; 2] = [0x55, 0xAA];
/// ```
pub const MBR_SIGNATURE: [u8; 2] = [0x55, 0xAA];
/// mbr standard size in bytes
/// # Value
/// ```
/// pub const MBR_SIZE : usize = 512;
/// ```
pub const MBR_SIZE: usize = 512;
pub const SECTOR_START: u64 = 0x3f;

/// mbr partition entry structure
#[derive(Debug, Default)]
pub struct MbrPartitionEntry {
    pub boot_indicator: u8,
    pub start_head: u8,
    pub start_sector: u8,
    pub start_cylinder: u8,
    pub partition_type: u8,
    pub end_head: u8,
    pub end_sector: u8,
    pub end_cylinder: u8,
    pub start_in_lba: u32,
    pub size_in_lba: u32,
}

fn lba_to_encoded_chs(lba: u32) -> (u8, u8, u8) {
    const HEADS_PER_CYLINDER: u32 = 255;
    const SECTORS_PER_TRACK: u32 = 63;

    let cylinder = lba / (HEADS_PER_CYLINDER * SECTORS_PER_TRACK);

    // conventional max CHS value.
    if cylinder > 1023 {
        return (0xFE, 0xFF, 0xFF);
    }

    let head = (lba / SECTORS_PER_TRACK) % HEADS_PER_CYLINDER;
    let sector = (lba % SECTORS_PER_TRACK) + 1;

    let head_byte = head as u8;
    let sector_byte = ((sector & 0x3F) as u8) | (((cylinder >> 2) & 0xC0) as u8);
    let cylinder_byte = (cylinder & 0xFF) as u8;

    (head_byte, sector_byte, cylinder_byte)
}

impl MbrPartitionEntry {
    pub fn new(partition_type: u8, start_in_lba: u32, size_in_lba: u32) -> Self {
        let (start_head, start_sector, start_cylinder) = lba_to_encoded_chs(start_in_lba);
        let (end_head, end_sector, end_cylinder) =
            lba_to_encoded_chs(start_in_lba + size_in_lba - 1);

        Self {
            boot_indicator: 0,
            start_head,
            start_sector,
            start_cylinder,
            partition_type,
            end_head,
            end_sector,
            end_cylinder,
            start_in_lba,
            size_in_lba,
        }
    }

    /// parse a single mbr partition entry from bytes
    pub fn from_bytes(bytes: &[u8]) -> MbrPartitionEntry {
        MbrPartitionEntry {
            boot_indicator: bytes[0],
            start_head: bytes[1],
            start_sector: bytes[2],
            start_cylinder: bytes[3],
            partition_type: bytes[4],
            end_head: bytes[5],
            end_sector: bytes[6],
            end_cylinder: bytes[7],
            start_in_lba: LittleEndian::read_u32(&bytes[8..12]),
            size_in_lba: LittleEndian::read_u32(&bytes[12..16]),
        }
    }

    pub fn to_bytes(&self) -> io::Result<[u8; 16]> {
        let mut buf: [u8; 16] = [
            self.boot_indicator,
            self.start_head,
            self.start_sector,
            self.start_cylinder,
            self.partition_type,
            self.end_head,
            self.end_sector,
            self.end_cylinder,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        LittleEndian::write_u32(&mut buf[8..12], self.start_in_lba);
        LittleEndian::write_u32(&mut buf[12..16], self.size_in_lba);
        Ok(buf)
    }
}

/// parse an mbr partition table
pub fn parse_partition_table(buffer: &[u8]) -> Result<Vec<MbrPartitionEntry>, io::Error> {
    if buffer[510..512] != MBR_SIGNATURE {
        return Err(io::Error::other("Bad mbr signature"));
    }
    let mut partition_table = Vec::new();
    for i in 0..4 {
        let raw_bytes = &buffer[446 + 16 * (i)..446 + 16 * (i + 1)];
        let entry = MbrPartitionEntry::from_bytes(raw_bytes);
        if entry.partition_type != 0x00 && entry.size_in_lba != 0 {
            partition_table.push(entry);
        }
    }
    Ok(partition_table)
}

pub fn write_partition<T>(file: &mut T, partition: &MbrPartitionEntry) -> io::Result<()>
where
    T: std::io::Seek + std::io::Write,
{
    file.write_all(&partition.to_bytes()?)
}
