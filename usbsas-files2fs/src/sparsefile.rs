use bitvec::prelude::*;
use std::io::{self, prelude::*};

/// One bit per sector, set if the sector was written.
pub type FileBitVec = BitVec<u8, Lsb0>;

/// Wrapper around a file recording which sectors are written.
pub struct SparseFile<T: Read + Write + Seek> {
    sector_size: u64,
    file: T,
    bitvec: FileBitVec,
}

impl<T: Read + Write + Seek> SparseFile<T> {
    /// Wrap `file`, made of `sector_count` sectors of `sector_size` bytes.
    pub fn new(file: T, sector_size: u64, sector_count: usize) -> Result<SparseFile<T>, io::Error> {
        let mut bitvec = BitVec::<u8, Lsb0>::new();
        bitvec.resize(sector_count, false);
        Ok(SparseFile {
            sector_size,
            file,
            bitvec,
        })
    }

    /// Flush and close the file, return the bit vector of written sectors.
    pub fn get_bitvec(mut self) -> Result<FileBitVec, io::Error> {
        self.file.flush()?;
        drop(self.file);
        Ok(self.bitvec)
    }
}

impl<T: Read + Write + Seek> Read for SparseFile<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

/// Writes mark the sectors they touch, writing past the last sector is an
/// error.
impl<T: Read + Write + Seek> Write for SparseFile<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let offset = self.file.stream_position()?;
        match self.file.write(buf)? {
            0 => Ok(0),
            size => {
                let sector_start = (offset / self.sector_size) as usize;
                let sector_stop = (offset + size as u64).div_ceil(self.sector_size) as usize;
                if sector_stop > self.bitvec.len() {
                    return Err(io::Error::new(io::ErrorKind::StorageFull, "bitvec full"));
                }
                self.bitvec[sector_start..sector_stop].fill(true);
                Ok(size)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl<T: Read + Write + Seek> Seek for SparseFile<T> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, SeekFrom};

    const SECTOR_SIZE: u64 = 512;

    fn sparse(sectors: usize) -> SparseFile<Cursor<Vec<u8>>> {
        SparseFile::new(Cursor::new(vec![0; sectors * SECTOR_SIZE as usize]), SECTOR_SIZE, sectors).unwrap()
    }

    fn written(bitvec: &FileBitVec) -> Vec<usize> {
        bitvec.iter_ones().collect()
    }

    #[test]
    fn nothing_written() {
        let bitvec = sparse(8).get_bitvec().unwrap();
        assert_eq!(bitvec.len(), 8);
        assert!(bitvec.not_any());
    }

    #[test]
    fn marks_touched_sectors() {
        let mut f = sparse(8);
        // Inside sector 1
        f.seek(SeekFrom::Start(SECTOR_SIZE + 10)).unwrap();
        f.write_all(b"a").unwrap();
        // Across sectors 3 and 4
        f.seek(SeekFrom::Start(4 * SECTOR_SIZE - 1)).unwrap();
        f.write_all(b"bc").unwrap();
        // Exactly sector 6
        f.seek(SeekFrom::Start(6 * SECTOR_SIZE)).unwrap();
        f.write_all(&[1; SECTOR_SIZE as usize]).unwrap();
        assert_eq!(written(&f.get_bitvec().unwrap()), [1, 3, 4, 6]);
    }

    #[test]
    fn writes_go_to_the_file() {
        let mut f = sparse(2);
        f.seek(SeekFrom::Start(SECTOR_SIZE)).unwrap();
        f.write_all(b"usbsas").unwrap();
        f.seek(SeekFrom::Start(SECTOR_SIZE)).unwrap();
        let mut buf = [0; 6];
        f.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"usbsas");
    }

    #[test]
    fn last_sector() {
        let mut f = sparse(4);
        f.seek(SeekFrom::Start(4 * SECTOR_SIZE - 2)).unwrap();
        f.write_all(b"ok").unwrap();
        assert_eq!(written(&f.get_bitvec().unwrap()), [3]);
    }

    #[test]
    fn write_past_last_sector() {
        let mut f = sparse(4);
        f.seek(SeekFrom::Start(4 * SECTOR_SIZE - 1)).unwrap();
        let err = f.write_all(b"no").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::StorageFull);
    }

    #[test]
    fn empty_write() {
        let mut f = sparse(4);
        f.seek(SeekFrom::Start(SECTOR_SIZE)).unwrap();
        assert_eq!(f.write(b"").unwrap(), 0);
        assert!(f.get_bitvec().unwrap().not_any());
    }
}
