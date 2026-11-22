//! The merged shaders' bytes, kept in a file instead of in memory.
//!
//! The split merges every file's shaders by key and writes the package only at the end (the records are sorted by group, source and key, so no record can be written before the last
//! file is read). Holding the first copy of each of Grizzy's 21,990 shaders costs 127 MB of heap (a control section of 2-4 KB and a code area of 2-16 KB each); the merge
//! only ever needs a shader's bytes twice more: to compare a second copy of a key with the first (`same_shader`), and to cut its record when the package is written. So the bytes go to
//! a spill file the moment they are merged, and the map keeps `(offset, lengths)`; both later uses read them back (the file sits in the page cache, which is not part of the
//! process's footprint).
//!
//! The file is a temporary in the package folder, `.split-spill-<pid>`: appended to by one thread (the merge runs on the thread that collects the jobs' results, in job order), read
//! by the same thread, and removed when the [`Spill`] is dropped. A kill leaves it behind; the next split removes any `.split-spill-*` it finds ([`Spill::sweep`]).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// bytes collected before a write (the merge appends one shader at a time)
const BUF: usize = 4 << 20;

pub struct Spill {
    file: File,
    path: PathBuf,
    buf: Vec<u8>,
    /// bytes already in the file
    flushed: u64,
}

/// Where a shader's bytes are in the spill: the control section, then the code area, at `off`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At {
    pub off: u64,
    pub nctl: u32,
    pub ncode: u32,
}

impl Spill {
    /// Removes the spill files an earlier split of this folder left (a kill, a crash)
    pub fn sweep(dir: &Path) {
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with(".split-spill-") {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
    }

    pub fn create(dir: &Path) -> io::Result<Spill> {
        fs::create_dir_all(dir)?;
        Spill::sweep(dir);
        let path = dir.join(format!(".split-spill-{}", std::process::id()));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        Ok(Spill {
            file,
            path,
            buf: Vec::with_capacity(BUF),
            flushed: 0,
        })
    }

    /// Appends a shader and returns where it is
    pub fn append(&mut self, ctl: &[u8], code: &[u8]) -> io::Result<At> {
        let off = self.flushed + self.buf.len() as u64;
        if self.buf.len() + ctl.len() + code.len() > BUF {
            self.flush()?;
        }
        self.buf.extend_from_slice(ctl);
        self.buf.extend_from_slice(code);
        Ok(At {
            off,
            nctl: ctl.len() as u32,
            ncode: code.len() as u32,
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            self.file.seek(SeekFrom::Start(self.flushed))?;
            self.file.write_all(&self.buf)?;
            self.flushed += self.buf.len() as u64;
            self.buf.clear();
        }
        Ok(())
    }

    /// `n` bytes at `off` of the spill (clamped to what is there) into `out`, which is cleared first
    fn read_range(&mut self, off: u64, n: usize, out: &mut Vec<u8>) -> io::Result<()> {
        out.clear();
        if off + n as u64 > self.flushed {
            if off >= self.flushed {
                // all of it is still in the buffer
                let s = (off - self.flushed) as usize;
                out.extend_from_slice(&self.buf[s..s + n]);
                return Ok(());
            }
            self.flush()?;
        }
        self.file.seek(SeekFrom::Start(off))?;
        out.resize(n, 0);
        self.file.read_exact(out)?;
        Ok(())
    }

    /// A shader's control section and code area, in `out` (control first): `out[..nctl]` and `out[nctl..]`
    pub fn read(&mut self, at: At, out: &mut Vec<u8>) -> io::Result<()> {
        self.read_range(at.off, at.nctl as usize + at.ncode as usize, out)
    }

    /// The first `n` bytes of a shader's code area (or all of it, when it is shorter), in `out`
    pub fn read_code_prefix(&mut self, at: At, n: usize, out: &mut Vec<u8>) -> io::Result<()> {
        self.read_range(at.off + at.nctl as u64, n.min(at.ncode as usize), out)
    }
}

impl Drop for Spill {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ibuilder-spill-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn what_is_appended_is_read_back_before_and_after_a_flush() {
        let d = temp("rw");
        let mut s = Spill::create(&d).unwrap();
        let mut at = Vec::new();
        let mut want = Vec::new();
        // enough to cross the buffer size several times, with lengths that are not round
        for i in 0..40usize {
            let ctl: Vec<u8> = (0..700 + i).map(|j| (i + j) as u8).collect();
            let code: Vec<u8> = (0..(150_000 + 997 * i))
                .map(|j| (i * 7 + j) as u8)
                .collect();
            at.push(s.append(&ctl, &code).unwrap());
            want.push((ctl, code));
            // read the one just appended (still in the buffer, or flushed by this very append) and an old one
            let mut out = Vec::new();
            s.read(*at.last().unwrap(), &mut out).unwrap();
            assert_eq!(&out[..want[i].0.len()], &want[i].0[..]);
            assert_eq!(&out[want[i].0.len()..], &want[i].1[..]);
            let k = i / 2;
            s.read(at[k], &mut out).unwrap();
            assert_eq!(
                &out[..want[k].0.len()],
                &want[k].0[..],
                "an older shader, after more appends"
            );
        }
        // reading moves the file cursor: appends after reads still land at the end
        let ctl = vec![9u8; 33];
        let code = vec![5u8; 44];
        let a = s.append(&ctl, &code).unwrap();
        let mut out = Vec::new();
        s.read(at[3], &mut out).unwrap();
        let b = s.append(&ctl, &code).unwrap();
        s.read(a, &mut out).unwrap();
        assert_eq!((&out[..33], &out[33..]), (&ctl[..], &code[..]));
        s.read(b, &mut out).unwrap();
        assert_eq!((&out[..33], &out[33..]), (&ctl[..], &code[..]));
        // a code prefix, and the whole code when it is shorter than asked
        s.read_code_prefix(at[5], 0x68, &mut out).unwrap();
        assert_eq!(&out[..], &want[5].1[..0x68]);
        s.read_code_prefix(a, 0x68, &mut out).unwrap();
        assert_eq!(&out[..], &code[..]);
        let path = s.path.clone();
        assert!(path.exists());
        drop(s);
        assert!(!path.exists(), "the spill file goes with the Spill");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_spill_left_by_a_killed_split_is_swept() {
        let d = temp("sweep");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(".split-spill-1234"), b"old").unwrap();
        fs::write(d.join("ps_data.bin"), b"keep").unwrap();
        let s = Spill::create(&d).unwrap();
        assert!(!d.join(".split-spill-1234").exists());
        assert!(d.join("ps_data.bin").exists());
        drop(s);
        let _ = fs::remove_dir_all(&d);
    }
}
