use bitvec::prelude::*;
use std::io::{self, prelude::*};

pub type FileBitVec = BitVec<u8, Lsb0>;

pub struct SparseFile<T: Read + Write + Seek> {
    sector_size: u64,
    file: T,
    bitvec: FileBitVec,
}

impl<T: Read + Write + Seek> SparseFile<T> {
    pub fn new(file: T, sector_size: u64, sector_count: usize) -> Result<SparseFile<T>, io::Error> {
        let mut bitvec = BitVec::<u8, Lsb0>::new();
        bitvec.resize(sector_count, false);
        Ok(SparseFile {
            sector_size,
            file,
            bitvec,
        })
    }

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

impl<T: Read + Write + Seek> Write for SparseFile<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        /* Get file position */
        let offset = self.file.stream_position()?;

        match self.file.write(buf)? {
            0 => Ok(0),
            size => {
                let sector_start = (offset / self.sector_size) as usize;
                let sector_stop =
                    ((offset + size as u64 + self.sector_size - 1) / self.sector_size) as usize;
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
