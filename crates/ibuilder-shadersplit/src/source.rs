//! Where the bytes of a file come from: a range of a file on disk (a loose file, or a member of a `DMROMFS1` image) read with `pread`, or a slice in memory.
//!
//! The Python reads every file whole (`f.read()`, 1.8 GB resident for Grizzy's 2.5 GB RomFS); a Rust reader asks for what it needs. A 380 MB `.resS` full of
//! texture data is only *searched* (in 1 MiB chunks) and read whole only when a container magic is in it; a Unity serialized file is read through its metadata
//! and the objects it wants. No memory map: the crate forbids `unsafe`, and a map is one.

use std::fs::File;
use std::io;
use std::sync::Arc;

pub trait Source: Send + Sync {
    fn len(&self) -> u64;
    /// like `pread`: fills as much of `buf` as there is at `off` and returns how many bytes
    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<usize>;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `d[off:off+n]`, clamped to the data
    fn read(&self, off: u64, n: usize) -> io::Result<Vec<u8>> {
        let len = self.len();
        if off >= len {
            return Ok(Vec::new());
        }
        let n = n.min((len - off) as usize);
        let mut v = vec![0u8; n];
        let mut got = 0;
        while got < n {
            let k = self.read_at(off + got as u64, &mut v[got..])?;
            if k == 0 {
                v.truncate(got);
                break;
            }
            got += k;
        }
        Ok(v)
    }

    fn read_all(&self) -> io::Result<Vec<u8>> {
        self.read(0, self.len() as usize)
    }
}

pub struct MemSource<'a>(pub &'a [u8]);

impl Source for MemSource<'_> {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        let off = (off as usize).min(self.0.len());
        let n = buf.len().min(self.0.len() - off);
        buf[..n].copy_from_slice(&self.0[off..off + n]);
        Ok(n)
    }
}

/// A byte range of an open file (the whole file, or one member of an image)
#[derive(Clone)]
pub struct FileSource {
    file: Arc<File>,
    base: u64,
    len: u64,
}

impl FileSource {
    pub fn new(file: Arc<File>, base: u64, len: u64) -> FileSource {
        FileSource { file, base, len }
    }
    pub fn open(path: &std::path::Path) -> io::Result<FileSource> {
        let f = File::open(path)?;
        let len = f.metadata()?.len();
        Ok(FileSource {
            file: Arc::new(f),
            base: 0,
            len,
        })
    }
}

impl Source for FileSource {
    fn len(&self) -> u64 {
        self.len
    }
    #[cfg(unix)]
    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        use std::os::unix::fs::FileExt;
        if off >= self.len {
            return Ok(0);
        }
        let n = buf.len().min((self.len - off) as usize);
        loop {
            match self.file.read_at(&mut buf[..n], self.base + off) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                r => return r,
            }
        }
    }
    #[cfg(not(unix))]
    fn read_at(&self, _off: u64, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pread needs a unix target",
        ))
    }
}

/// The window a search reads at a time: 1 MiB (the searches are memory-bandwidth bound; a bigger window only makes every thread that searches hold more, 6 x 4 MiB before)
const SEARCH_CHUNK: usize = 1 << 20;

/// Which of `needles` occur in the source, searched in chunks of 1 MiB (with the overlap a needle needs) so that a huge file costs no memory.
/// Returns one flag per needle (it stops early once every needle was seen).
pub fn contains_each(src: &dyn Source, needles: &[&[u8]]) -> io::Result<Vec<bool>> {
    const CHUNK: usize = SEARCH_CHUNK;
    let mut seen = vec![false; needles.len()];
    let maxn = needles.iter().map(|n| n.len()).max().unwrap_or(1);
    let finders: Vec<memchr::memmem::Finder<'_>> =
        needles.iter().map(memchr::memmem::Finder::new).collect();
    let len = src.len();
    let mut buf = vec![0u8; CHUNK + maxn];
    let mut off = 0u64;
    while off < len {
        let want = ((len - off) as usize).min(CHUNK + maxn - 1);
        let mut got = 0;
        while got < want {
            let k = src.read_at(off + got as u64, &mut buf[got..want])?;
            if k == 0 {
                break;
            }
            got += k;
        }
        let w = &buf[..got];
        for (i, f) in finders.iter().enumerate() {
            if !seen[i] && f.find(w).is_some() {
                seen[i] = true;
            }
        }
        if seen.iter().all(|&s| s) || got < want {
            break;
        }
        off += CHUNK as u64;
    }
    Ok(seen)
}

/// `d.find(needle, from)` over a source, in chunks of 1 MiB with the overlap a needle needs
pub fn find_from(src: &dyn Source, needle: &[u8], from: u64) -> io::Result<Option<u64>> {
    const CHUNK: usize = SEARCH_CHUNK;
    let len = src.len();
    let mut off = from;
    let finder = memchr::memmem::Finder::new(needle);
    let mut buf = vec![0u8; CHUNK + needle.len()];
    while off < len {
        let want = ((len - off) as usize).min(CHUNK + needle.len() - 1);
        let mut got = 0;
        while got < want {
            let k = src.read_at(off + got as u64, &mut buf[got..want])?;
            if k == 0 {
                break;
            }
            got += k;
        }
        if let Some(p) = finder.find(&buf[..got]) {
            return Ok(Some(off + p as u64));
        }
        if got < want || off + CHUNK as u64 >= len {
            break;
        }
        off += CHUNK as u64;
    }
    Ok(None)
}
