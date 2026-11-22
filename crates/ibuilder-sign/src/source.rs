use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

/// Random-access, shareable read access to the image being signed (hashing runs on several threads).
pub trait Source: Sync {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Fill `buf` from `off`; an error when the range is not entirely inside the image.
    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<()>;
}

impl Source for [u8] {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<()> {
        let end = off
            .checked_add(buf.len() as u64)
            .filter(|&e| e <= <[u8]>::len(self) as u64);
        match end {
            Some(e) => {
                buf.copy_from_slice(&self[off as usize..e as usize]);
                Ok(())
            }
            None => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "read past the end of the image",
            )),
        }
    }
}

impl Source for Vec<u8> {
    fn len(&self) -> u64 {
        Vec::len(self) as u64
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<()> {
        self.as_slice().read_at(off, buf)
    }
}

/// A file: positional reads (`pread`), so several threads can read it at once.
pub(crate) struct FileSource {
    pub file: File,
    pub len: u64,
}

impl Source for FileSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<()> {
        self.file.read_exact_at(buf, off)
    }
}
