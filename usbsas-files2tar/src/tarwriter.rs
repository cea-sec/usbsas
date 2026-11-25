use crate::ArchiveWriter;
use crate::{Error, Result};
use std::{
    io::Write,
    path::{Component, Path},
};
use usbsas_proto::common::FileType;
use usbsas_utils::{TAR_BLOCK_SIZE, TAR_DATA_DIR};

/// Largest modification time of a ustar header (11 octal digits).
const MAX_MTIME: i64 = 0o777_7777_7777;

/// Tar (ustar) [`ArchiveWriter`].
pub(crate) struct TarWriter<W: Write> {
    builder: tar::Builder<W>,
    data_dir: String,
}

impl<W: Write> TarWriter<W> {
    pub(crate) fn new(writer: W) -> Self {
        TarWriter {
            builder: tar::Builder::new(writer),
            data_dir: TAR_DATA_DIR.trim_end_matches('/').to_owned() + "/",
        }
    }
}

impl<W: Write> ArchiveWriter for TarWriter<W> {
    fn init(&mut self) -> Result<()> {
        self.builder.follow_symlinks(false);
        let mut header = tar::Header::new_ustar();
        header.set_size(0);
        header.set_entry_type(tar::EntryType::Directory);
        header.set_mode(0o755);
        header.set_path(self.data_dir.clone())?;
        header.set_cksum();
        self.builder.append(&header, std::io::empty())?;
        Ok(())
    }

    /// Files are stored under the data directory, except metadata files.
    /// Paths containing `..` are refused, and files need a name (but the root
    /// directory `/` is accepted, written as the data directory).
    fn newfile(&mut self, path: &str, ftype: FileType, size: u64, timestamp: i64) -> Result<()> {
        let mut path_string: String = path.trim_start_matches('/').into();
        let mut components = Path::new(&path_string).components();
        if components.clone().any(|c| c == Component::ParentDir) {
            return Err(Error::Error("Bad file name".to_string()));
        }
        if ftype != FileType::Directory && !components.any(|c| matches!(c, Component::Normal(_))) {
            return Err(Error::Error("Bad file name".to_string()));
        }
        let mut header = tar::Header::new_ustar();
        match ftype {
            FileType::Regular | FileType::Metadata => {
                header.set_size(size);
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(0o644);
            }
            FileType::Directory => {
                header.set_size(0);
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(0o755);
            }
            _ => return Err(Error::Error("Bad file type".to_string())),
        }
        // Times before 1970 (or after 2242) are clamped
        header.set_mtime(timestamp.clamp(0, MAX_MTIME) as u64);
        if !matches!(ftype, FileType::Metadata) {
            path_string.insert_str(0, &self.data_dir);
        }
        self.builder
            .append_data(&mut header, Path::new(&path_string), std::io::empty())?;
        Ok(())
    }

    fn writefile(&mut self, data: &[u8]) -> Result<()> {
        self.builder.get_mut().write_all(data)?;
        Ok(())
    }

    /// Pad the file data to a tar block.
    fn endfile(&mut self, len_written: usize) -> Result<()> {
        // Pad to size of block
        let buf = [0; TAR_BLOCK_SIZE];
        let remaining = TAR_BLOCK_SIZE - (len_written % TAR_BLOCK_SIZE);
        if remaining < TAR_BLOCK_SIZE {
            self.builder.get_mut().write_all(&buf[..remaining])?;
        }
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<()> {
        // Make sure everything is flushed and closed
        let mut inner = self.builder.into_inner()?;
        inner.flush()?;
        drop(inner);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Entry read back: path, type, size, mtime and content.
    type Entry = (String, tar::EntryType, u64, u64, Vec<u8>);

    fn read_back(tar: &[u8]) -> Vec<Entry> {
        tar::Archive::new(tar)
            .entries()
            .unwrap()
            .map(|e| {
                let mut e = e.unwrap();
                let mut data = Vec::new();
                e.read_to_end(&mut data).unwrap();
                let h = e.header();
                (
                    e.path().unwrap().to_string_lossy().into_owned(),
                    h.entry_type(),
                    h.size().unwrap(),
                    h.mtime().unwrap(),
                    data,
                )
            })
            .collect()
    }

    fn writer() -> TarWriter<Vec<u8>> {
        let mut w = TarWriter::new(Vec::new());
        w.init().unwrap();
        w
    }

    fn file(w: &mut TarWriter<Vec<u8>>, path: &str, ftype: FileType, data: &[u8]) {
        w.newfile(path, ftype, data.len() as u64, 1_600_000_000)
            .unwrap();
        for chunk in data.chunks(100) {
            w.writefile(chunk).unwrap();
        }
        w.endfile(data.len()).unwrap();
    }

    fn finish(w: TarWriter<Vec<u8>>) -> Vec<u8> {
        w.builder.into_inner().unwrap()
    }

    #[test]
    fn layout() {
        let mut w = writer();
        w.newfile("/dir", FileType::Directory, 0, 1_500_000_000)
            .unwrap();
        w.endfile(0).unwrap();
        file(&mut w, "/dir/a.txt", FileType::Regular, b"hello");
        file(&mut w, "/empty", FileType::Regular, b"");
        // Exactly one block: no padding
        file(&mut w, "/block", FileType::Regular, &[7; TAR_BLOCK_SIZE]);
        file(&mut w, "config.json", FileType::Metadata, b"{}");
        let tar = finish(w);
        assert_eq!(tar.len() % TAR_BLOCK_SIZE, 0);

        let entries = read_back(&tar);
        let summary: Vec<_> = entries
            .iter()
            .map(|(p, t, s, _, d)| (p.as_str(), *t, *s, d.as_slice()))
            .collect();
        assert_eq!(
            summary,
            [
                ("data/", tar::EntryType::Directory, 0, &b""[..]),
                ("data/dir", tar::EntryType::Directory, 0, b""),
                ("data/dir/a.txt", tar::EntryType::Regular, 5, b"hello"),
                ("data/empty", tar::EntryType::Regular, 0, b""),
                ("data/block", tar::EntryType::Regular, 512, &[7; 512]),
                // Metadata files are at the root of the archive
                ("config.json", tar::EntryType::Regular, 2, b"{}"),
            ]
        );
        assert_eq!(entries[1].3, 1_500_000_000);
        assert_eq!(entries[2].3, 1_600_000_000);
    }

    #[test]
    fn long_path() {
        let path = format!("/{}/{}.txt", "d".repeat(150), "f".repeat(150));
        let mut w = writer();
        file(&mut w, &path, FileType::Regular, b"long");
        let entries = read_back(&finish(w));
        assert_eq!(entries[1].0, format!("data{path}"));
        assert_eq!(entries[1].4, b"long");
    }

    #[test]
    fn bad_names() {
        for path in ["..", "/..", "../a", "/a/../../b", "a/..", "a/../b"] {
            for ftype in [FileType::Regular, FileType::Directory, FileType::Metadata] {
                let mut w = writer();
                assert!(w.newfile(path, ftype, 0, 0).is_err(), "{path:?} accepted");
                // Nothing written after the data directory
                assert_eq!(read_back(&finish(w)).len(), 1);
            }
        }
    }

    #[test]
    fn root() {
        for path in ["", "/", "//", ".", "/./"] {
            // The root directory is the data directory
            let mut w = writer();
            w.newfile(path, FileType::Directory, 0, 0).unwrap();
            w.endfile(0).unwrap();
            let entries = read_back(&finish(w));
            assert_eq!(entries.len(), 2);
            assert!(
                entries[1].0 == "data/" || entries[1].0 == "data",
                "{path:?}"
            );
            assert_eq!(entries[1].1, tar::EntryType::Directory);
            // A file needs a name
            for ftype in [FileType::Regular, FileType::Metadata] {
                let mut w = writer();
                assert!(w.newfile(path, ftype, 0, 0).is_err(), "{path:?} accepted");
                assert_eq!(read_back(&finish(w)).len(), 1);
            }
        }
    }

    #[test]
    fn current_dir_components_ignored() {
        let mut w = writer();
        file(&mut w, "/./a/./b", FileType::Regular, b"x");
        assert_eq!(read_back(&finish(w))[1].0, "data/a/b");
    }

    #[test]
    fn timestamps_clamped() {
        for (ts, expected) in [
            (-1, 0),
            (i64::MIN, 0),
            (0, 0),
            (MAX_MTIME, MAX_MTIME as u64),
            (MAX_MTIME + 1, MAX_MTIME as u64),
            (i64::MAX, MAX_MTIME as u64),
        ] {
            let mut w = writer();
            w.newfile("/a", FileType::Regular, 0, ts).unwrap();
            w.endfile(0).unwrap();
            let tar = finish(w);
            assert_eq!(read_back(&tar)[1].3, expected, "{ts}");
            // Standard octal field, readable by any ustar reader
            let mtime_field = &tar[TAR_BLOCK_SIZE + 136..TAR_BLOCK_SIZE + 148];
            assert!(mtime_field[0] & 0x80 == 0, "{ts}: binary encoding");
        }
    }

    #[test]
    fn bad_type() {
        let mut w = writer();
        assert!(w.newfile("/a", FileType::Other, 0, 0).is_err());
    }
}
