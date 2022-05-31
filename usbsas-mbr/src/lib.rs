//! Master boot record parser

use byteorder::{ByteOrder, LittleEndian};
use log::error;
use packed_struct::prelude::*;

use std::io::{self, ErrorKind};

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
#[derive(Debug, Default, PackedStruct)]
#[packed_struct(endian = "lsb")]
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

impl MbrPartitionEntry {
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
        self.pack().map_err(|err| {
            error!("pack() error");
            io::Error::new(io::ErrorKind::Other, err)
        })
    }
}

/// parse an mbr partition table
pub fn parse_partition_table(buffer: &[u8]) -> Result<Vec<MbrPartitionEntry>, io::Error> {
    if buffer[510..512] != MBR_SIGNATURE {
        return Err(io::Error::new(ErrorKind::Other, "Bad mbr signature"));
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
