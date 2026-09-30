use crate::FSRead;
use crate::{Error, Result};
use hadris_iso::directory::DirectoryRecordHeader;
use hadris_iso::file::EntryType;
use hadris_iso::read::{DirEntry, IsoImage};
use std::io::{Read, Seek};
use usbsas_proto::common::{FileInfo, FileType};

pub struct Iso9660<T: Read + Seek> {
    fs: IsoImage<T>,
    // Joliet names are UTF-16BE and need decoding differently from the other
    // (ASCII-based) interchange levels.
    joliet: bool,
}

fn find_entry<T: Read + Seek>(fs: &IsoImage<T>, path: &str) -> Result<DirEntry> {
    fs.find_path(path)?
        .ok_or_else(|| Error::Error("couldn't find path".to_string()))
}

// Directory records store filenames with a trailing ";<version>", strip this
fn strip_version(name: &str) -> &str {
    match name.rsplit_once(';') {
        Some((base, version))
            if !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit()) =>
        {
            base
        }
        _ => name,
    }
}

fn entry_name(entry: &DirEntry, joliet: bool) -> String {
    let raw = if joliet {
        entry.record.joliet_name()
    } else {
        entry.display_name().into_owned()
    };
    strip_version(raw.trim_end_matches('\0')).to_string()
}

fn record_timestamp(header: &DirectoryRecordHeader) -> i64 {
    // DirDateTime doesn't expose its fields, but it's a bytemuck::Pod so its
    // raw on-disk bytes (ECMA-119 9.1.5) can be read directly.
    let raw = bytemuck::bytes_of(&header.date_time);
    let (year, month, day, hour, minute, second, gmt_offset) =
        (raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6] as i8);

    let date = time::Date::from_calendar_date(
        1900 + year as i32,
        time::Month::try_from(month).unwrap_or(time::Month::January),
        day,
    )
    .unwrap_or_else(|_| time::Date::from_calendar_date(0, time::Month::January, 1).unwrap());
    let time_of_day = time::Time::from_hms(hour, minute, second)
        .unwrap_or_else(|_| time::Time::from_hms(0, 0, 0).unwrap());
    let offset = time::UtcOffset::from_whole_seconds((gmt_offset as i32) * 15 * 60)
        .unwrap_or(time::UtcOffset::UTC);

    time::PrimitiveDateTime::new(date, time_of_day)
        .assume_offset(offset)
        .unix_timestamp()
}

impl<T: Read + Seek> FSRead<T> for Iso9660<T> {
    fn new(reader: T, _sector_size: u32) -> Result<Self> {
        let fs = IsoImage::open(reader)?;
        let joliet = matches!(fs.root_dir().entry_type(), EntryType::Joliet { .. });
        Ok(Iso9660 { fs, joliet })
    }

    fn get_attr(&mut self, path: &str) -> Result<(FileType, u64, i64)> {
        log::trace!("get_attr: '{path}'");
        if path.is_empty() || path == "/" {
            let pvd = self.fs.read_pvd()?;
            return Ok((
                FileType::Directory,
                0,
                record_timestamp(&pvd.dir_record.header),
            ));
        }
        let entry = find_entry(&self.fs, path)?;
        let ts = record_timestamp(entry.header());
        let (ftype, size) = if entry.is_directory() {
            (FileType::Directory, 0)
        } else {
            (FileType::Regular, entry.total_size())
        };
        Ok((ftype, size, ts))
    }

    fn read_dir(&mut self, path: &str) -> Result<Vec<FileInfo>> {
        log::trace!("read_dir: '{path}'");
        let dir_ref = if path.trim_matches('/').is_empty() {
            self.fs.root_dir().dir_ref()
        } else {
            let entry = find_entry(&self.fs, path)?;
            if !entry.is_directory() {
                return Err(Error::Error("{path} is not a directory".to_string()));
            }
            entry.as_dir_ref(&self.fs)?
        };

        let mut entries: Vec<FileInfo> = Vec::new();
        for entry in self.fs.open_dir(dir_ref).read_entries()? {
            if entry.is_special() {
                continue;
            }
            let name = entry_name(&entry, self.joliet);
            let full_name = format!("{}/{name}", path.trim_end_matches('/'));
            let ts = record_timestamp(entry.header());
            let (ftype, size) = if entry.is_directory() {
                (FileType::Directory, 0)
            } else {
                (FileType::Regular, entry.total_size())
            };
            entries.push(FileInfo {
                path: full_name,
                size,
                ftype: ftype.into(),
                timestamp: ts,
            });
        }
        Ok(entries)
    }

    fn read_file(
        &mut self,
        path: &str,
        buf: &mut Vec<u8>,
        offset: u64,
        bytes_to_read: u64,
    ) -> Result<u64> {
        log::trace!("read_file: '{path}'");
        let entry = find_entry(&self.fs, path)?;
        let total = entry.total_size();
        if offset >= total {
            return Ok(0);
        }
        let to_read = bytes_to_read.min(total - offset).min(buf.len() as u64) as usize;

        let mut remaining = to_read;
        let mut buf_pos = 0usize;
        let mut cur_offset = offset;
        let mut extent_start = 0u64;
        for extent in entry.extents() {
            if remaining == 0 {
                break;
            }
            let extent_len = extent.length as u64;
            let extent_end = extent_start + extent_len;
            if cur_offset < extent_end {
                let skip_in_extent = cur_offset - extent_start;
                let avail_in_extent = extent_len - skip_in_extent;
                let read_len = avail_in_extent.min(remaining as u64) as usize;
                let byte_offset = extent.sector.0 as u64 * 2048 + skip_in_extent;
                self.fs
                    .read_bytes_at(byte_offset, &mut buf[buf_pos..buf_pos + read_len])?;
                buf_pos += read_len;
                remaining -= read_len;
                cur_offset += read_len as u64;
            }
            extent_start = extent_end;
        }
        Ok(buf_pos as u64)
    }

    fn unmount_fs(self: Box<Self>) -> Result<T> {
        Ok(self.fs.into_inner())
    }
}
