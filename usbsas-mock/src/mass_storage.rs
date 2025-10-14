use positioned_io2::ReadAt;
use std::{
    fs::File,
    io::{self, ErrorKind, Read, Seek, SeekFrom, Write},
};

pub struct MockMassStorage {
    fakedev: File,
    pub block_size: u32,
    pub dev_size: u64,
    pub pos: u64,
}

impl MockMassStorage {
    pub fn from_opened_file(file: File) -> Result<Self, io::Error> {
        let dev_size = file.metadata()?.len();
        Ok(MockMassStorage {
            fakedev: file,
            block_size: 512,
            dev_size,
            pos: 0,
        })
    }

    pub fn read_sectors(
        &mut self,
        offset: u64,
        count: u64,
        block_size: usize,
    ) -> Result<Vec<u8>, io::Error> {
        self.fakedev
            .seek(SeekFrom::Start((offset as usize * block_size) as u64))?;
        let mut buf = vec![0; count as usize * block_size];
        self.fakedev.read_exact(&mut buf)?;
        Ok(buf)
    }

    pub fn scsi_write_10(
        &mut self,
        buffer: &mut [u8],
        offset: u64,
        _: u64,
    ) -> Result<u8, io::Error> {
        self.fakedev
            .seek(SeekFrom::Start(offset * self.block_size as u64))?;
        self.fakedev.write_all(buffer)?;
        Ok(0)
    }
}

impl Read for MockMassStorage {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.pos.is_multiple_of(self.block_size as u64) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Read on non sector aligned",
            ));
        }
        if !buf.len().is_multiple_of(self.block_size as usize) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Read on non sector size",
            ));
        }
        let offset = self.pos / self.block_size as u64;
        let sectors = buf.len() as u64 / self.block_size as u64;

        let data = self.read_sectors(offset, sectors, self.block_size as usize)?;
        self.pos += data.len() as u64;
        for (i, c) in data.iter().enumerate() {
            buf[i] = *c;
        }
        Ok(buf.len())
    }
}

impl Seek for MockMassStorage {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(pos) => {
                self.pos = pos;
                Ok(self.pos)
            }
            _ => Err(io::Error::new(ErrorKind::InvalidInput, "unsupported seek")),
        }
    }
}

impl ReadAt for MockMassStorage {
    fn read_at(&self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        if !self.pos.is_multiple_of(self.block_size as u64) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Read on non sector aligned",
            ));
        }
        if !buf.len().is_multiple_of(self.block_size as usize) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Read on non sector size",
            ));
        }
        self.fakedev.read_at(pos, buf)
    }

    fn read_exact_at(&self, pos: u64, buf: &mut [u8]) -> io::Result<()> {
        if !self.pos.is_multiple_of(self.block_size as u64) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Read on non sector aligned",
            ));
        }
        if !buf.len().is_multiple_of(self.block_size as usize) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Read on non sector size",
            ));
        }
        self.fakedev.read_exact_at(pos, buf)
    }
}
