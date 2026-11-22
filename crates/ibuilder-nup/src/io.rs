//! Positioned reads: the container is never mapped (it is tens of GB) and never read through a shared cursor, so several threads
//! read different files of it at once.

use std::io;

pub trait ReadAt: Send + Sync {
    fn len(&self) -> u64;
    /// as `pread`: how many bytes were read at `offset` (0 at the end)
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize>;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn read_exact_at(&self, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
        while !buf.is_empty() {
            match self.read_at(buf, offset) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "unexpected end of the container",
                    ));
                }
                Ok(n) => {
                    buf = &mut buf[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

/// A file on disk, read with `pread`.
pub struct FileSource {
    file: std::fs::File,
    len: u64,
}

impl FileSource {
    pub fn open(path: &std::path::Path) -> io::Result<FileSource> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        Ok(FileSource { file, len })
    }
}

impl ReadAt for FileSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        use std::os::unix::fs::FileExt;
        self.file.read_at(buf, offset)
    }
}

/// A container in memory (tests, small synthetic containers).
pub struct MemSource(pub Vec<u8>);

impl ReadAt for MemSource {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let o = (offset as usize).min(self.0.len());
        let n = buf.len().min(self.0.len() - o);
        buf[..n].copy_from_slice(&self.0[o..o + n]);
        Ok(n)
    }
}

/// `Read` over a byte range of a [`ReadAt`] (one file's extent), with a buffer of its own.
pub(crate) struct Extent<'a> {
    pub src: &'a dyn ReadAt,
    pub pos: u64,
    pub end: u64,
}

impl io::Read for Extent<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let want = (self.end - self.pos).min(buf.len() as u64) as usize;
        if want == 0 {
            return Ok(0);
        }
        let n = self.src.read_at(&mut buf[..want], self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}
