use std::io::{Error, ErrorKind, Read, Result, Seek, SeekFrom, Write};

/// ioctl() functions that will be called in ff extern callback functions
pub(crate) trait Ioctl {
    fn sector_size(&self) -> u32;
}

pub(crate) trait WrapperFatFs: Read + Write + Seek + Ioctl {}
impl<T: Read + Seek> WrapperFatFs for WrapperRead<T> {}
impl<T: Read + Write + Seek> WrapperFatFs for WrapperReadWrite<T> {}

/// Wrapper for T: Read + Seek to impl Ioctl
// (and also Write so we can impl WrapperFatFs) This is needed because in extern
// C functions we need a Box<dyn WrapperFatFs> But if FatFs<T> struct was
// initialized with the new() fn (and not mkfs) that only requires Read + Seek,
// ff will never do write operations so it's "ok".
pub(crate) struct WrapperRead<T> {
    inner: T,
    sector_size: u32,
}

impl<T> WrapperRead<T> {
    pub(crate) fn new(inner: T, sector_size: u32) -> Self {
        WrapperRead { inner, sector_size }
    }

    pub(crate) fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: Read> Read for WrapperRead<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.read(buf)
    }
}

impl<T: Seek> Seek for WrapperRead<T> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.inner.seek(pos)
    }
}

impl<T> Write for WrapperRead<T> {
    fn write(&mut self, _buf: &[u8]) -> Result<usize> {
        Err(Error::new(ErrorKind::PermissionDenied, "Read Only"))
    }

    fn flush(&mut self) -> Result<()> {
        Err(Error::new(ErrorKind::PermissionDenied, "Read Only"))
    }
}

impl<T> Ioctl for WrapperRead<T> {
    fn sector_size(&self) -> u32 {
        self.sector_size
    }
}

/// Wrapper for T: Read + Write + Seek to impl Ioctl
pub(crate) struct WrapperReadWrite<T> {
    inner: T,
    sector_size: u32,
}

impl<T> WrapperReadWrite<T> {
    pub(crate) fn new(inner: T, sector_size: u32) -> Self {
        WrapperReadWrite { inner, sector_size }
    }

    pub(crate) fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: Read> Read for WrapperReadWrite<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.read(buf)
    }
}

impl<T: Seek> Seek for WrapperReadWrite<T> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.inner.seek(pos)
    }
}

impl<T: Write> Write for WrapperReadWrite<T> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        self.inner.flush()
    }
}

impl<T> Ioctl for WrapperReadWrite<T> {
    fn sector_size(&self) -> u32 {
        self.sector_size
    }
}
