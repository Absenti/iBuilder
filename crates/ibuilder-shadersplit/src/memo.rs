//! `JobMemo`: the split's finished jobs kept on disk, so a split the OS ended half way (iOS ends the app for memory: SIGKILL, nothing logged) does not start over.
//!
//! The Python pickles each job's result under a digest of the job (the function's name and the repr of its arguments); a later run of the same split takes it from there. A file is
//! written whole (tmp, then rename), so a kill leaves a job done or not done, never half. The caller owns what the folder is valid for (same game files, DLC, split version).
//! The format here is this crate's own (`SSMEMO01`): the same results, not Python pickles; a folder left by the Python app is not read (`pipeline`: the signature includes the split version).

use crate::ctl::St;
use crate::scan_types::{Extra, FileResult, Pair, Shader};
use crate::unity::{Node, Trees};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const MAGIC: &[u8; 8] = b"SSMEMO01";

pub struct JobMemo {
    folder: PathBuf,
    pub hits: AtomicUsize,
    pub runs: AtomicUsize,
}

impl JobMemo {
    /// Opens (creating) the folder; what a kill left half written is removed
    pub fn open(folder: &Path) -> std::io::Result<JobMemo> {
        fs::create_dir_all(folder)?;
        for e in fs::read_dir(folder)? {
            let e = e?;
            if e.file_name().to_string_lossy().contains(".tmp") {
                let _ = fs::remove_file(e.path());
            }
        }
        Ok(JobMemo {
            folder: folder.to_path_buf(),
            hits: AtomicUsize::new(0),
            runs: AtomicUsize::new(0),
        })
    }

    pub fn key(parts: &[&str]) -> String {
        let mut h = Sha256::new();
        for p in parts {
            h.update((p.len() as u64).to_le_bytes());
            h.update(p.as_bytes());
        }
        crate::bytes::hex(&h.finalize()[..16])
    }

    fn path(&self, key: &str) -> PathBuf {
        self.folder.join(format!("{key}.job"))
    }

    /// The result of a finished job, or None (absent or damaged: the job runs again). The file is decoded as it is read: a job's result is never held twice (raw bytes and decoded).
    pub fn get(&self, key: &str) -> Option<Vec<FileResult>> {
        let p = self.path(key);
        let f = fs::File::open(&p).ok()?;
        let len = f.metadata().ok()?.len();
        match decode_from(std::io::BufReader::with_capacity(1 << 20, f), len) {
            Some(v) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(v)
            }
            None => {
                let _ = fs::remove_file(&p);
                None
            }
        }
    }

    /// Keeps a finished job's result. The file is encoded as it is written: the result is not copied into one more buffer first.
    pub fn put(&self, key: &str, value: &[FileResult]) {
        let path = self.path(key);
        let tmp = self.folder.join(format!(
            "{key}.job.tmp-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let r = (|| -> std::io::Result<()> {
            let mut w = std::io::BufWriter::with_capacity(1 << 20, fs::File::create(&tmp)?);
            encode_to(&mut w, value)?;
            w.flush()?;
            drop(w);
            fs::rename(&tmp, &path)
        })();
        if r.is_err() {
            let _ = fs::remove_file(&tmp); // a full drive: the job just is not kept
        }
        self.runs.fetch_add(1, Ordering::Relaxed);
    }
}

/// Writes `path` whole or not at all (review r2, N7): what `fill` writes goes to `<path>.part` in the same folder, and that is renamed over `path` once it is complete and flushed. On any failure the
/// `.part` is removed and the old `path` is as it was, so a file a later step reads is never half written (a kill leaves a `.part`, which the next write replaces). A `.part` that cannot be created or
/// a `path` that cannot be replaced (a directory in the way) is the `Err`.
pub(crate) fn write_atomic(
    path: &Path,
    fill: impl FnOnce(&mut std::io::BufWriter<fs::File>) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    let part = PathBuf::from(name);
    let r = (|| {
        let mut w = std::io::BufWriter::new(fs::File::create(&part)?);
        fill(&mut w)?;
        w.flush()?;
        drop(w);
        fs::rename(&part, path)
    })();
    if r.is_err() {
        let _ = fs::remove_file(&part);
    }
    r
}

// ---- encoding ---------------------------------------------------------------------------------------------------------------

/// The file's writer: the first write error is kept and the rest of the encoding is skipped (the encoder itself never fails)
struct W<'a, T: Write> {
    out: &'a mut T,
    err: Option<std::io::Error>,
}
impl<T: Write> W<'_, T> {
    fn raw(&mut self, b: &[u8]) {
        if self.err.is_none()
            && let Err(e) = self.out.write_all(b)
        {
            self.err = Some(e);
        }
    }
    fn u8(&mut self, v: u8) {
        self.raw(&[v])
    }
    fn u32(&mut self, v: u32) {
        self.raw(&v.to_le_bytes())
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.raw(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes())
    }
}

fn node<T: Write>(w: &mut W<'_, T>, n: &Node) {
    w.str(&n.ty);
    w.str(&n.name);
    w.u32(n.flags as u32);
    w.u32(n.kids.len() as u32);
    for k in &n.kids {
        node(w, k);
    }
}

/// The memo file of a job's results, written to `out`
pub fn encode_to<T: Write>(out: &mut T, rs: &[FileResult]) -> std::io::Result<()> {
    let mut w = W { out, err: None };
    w.raw(MAGIC);
    w.u32(rs.len() as u32);
    for r in rs {
        w.str(&r.rel);
        w.u32(r.uniq.len() as u32);
        for (k, s) in &r.uniq {
            w.str(k);
            w.str(&s.label);
            w.u8(s.st as u8);
            w.bytes(&s.ctl);
            w.bytes(&s.code);
        }
        w.u32(r.pairs.len() as u32);
        for p in &r.pairs {
            w.str(&p.a);
            w.str(&p.b);
            w.u8(p.prog as u8);
        }
        match &r.extra {
            Extra::Trees(t) => {
                w.u8(0);
                w.u32(t.len() as u32);
                let mut ks: Vec<_> = t.iter().collect();
                ks.sort_by_key(|(k, _)| **k);
                for ((cid, h), root) in ks {
                    w.u32(*cid as u32);
                    w.raw(h);
                    match root {
                        Some(n) => {
                            w.u8(1);
                            node(&mut w, n);
                        }
                        None => w.u8(0),
                    }
                }
            }
            Extra::Defer => w.u8(1),
            Extra::Unreadable { why, count } => {
                w.u8(2);
                w.str(why);
                w.u32(*count as u32);
            }
        }
    }
    match w.err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

pub fn encode(rs: &[FileResult]) -> Vec<u8> {
    let mut v = Vec::new();
    let _ = encode_to(&mut v, rs); // a Vec does not fail
    v
}

/// What a damaged file must not be able to make this reader do: allocate what it claims
struct R<T: Read> {
    inp: T,
    /// bytes of the file not yet read: a length field larger than this is damage, whatever it says
    left: u64,
}
impl<T: Read> R<T> {
    fn take(&mut self, n: usize) -> Option<Vec<u8>> {
        if n as u64 > self.left {
            return None;
        }
        let mut v = vec![0u8; n];
        self.inp.read_exact(&mut v).ok()?;
        self.left -= n as u64;
        Some(v)
    }
    fn fixed<const N: usize>(&mut self) -> Option<[u8; N]> {
        if N as u64 > self.left {
            return None;
        }
        let mut b = [0u8; N];
        self.inp.read_exact(&mut b).ok()?;
        self.left -= N as u64;
        Some(b)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.fixed::<1>()?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.fixed::<4>()?))
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn str(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?).ok()
    }
}

fn read_node<T: Read>(r: &mut R<T>, depth: usize) -> Option<Node> {
    if depth > 64 {
        return None;
    }
    let (ty, name, flags, n) = (r.str()?, r.str()?, r.u32()? as i32, r.u32()? as usize);
    let mut kids = Vec::new();
    for _ in 0..n {
        kids.push(read_node(r, depth + 1)?);
    }
    Some(Node {
        ty,
        name,
        flags,
        kids,
    })
}

/// A job's results from a memo file of `len` bytes, read as it is decoded; None when the file is damaged (any way)
pub fn decode_from<T: Read>(inp: T, len: u64) -> Option<Vec<FileResult>> {
    let mut r = R { inp, left: len };
    if &r.fixed::<8>()? != MAGIC {
        return None;
    }
    let n = r.u32()? as usize;
    let mut out = Vec::new();
    for _ in 0..n {
        let rel = r.str()?;
        let nu = r.u32()? as usize;
        let mut uniq = Vec::new();
        for _ in 0..nu {
            let key = r.str()?;
            let label = r.str()?;
            let st = match r.u8()? {
                0 => St::Vs,
                1 => St::Hs,
                2 => St::Ds,
                3 => St::Gs,
                4 => St::Ps,
                5 => St::Cs,
                6 => St::Ts,
                7 => St::Ms,
                8 => St::Rt,
                _ => return None,
            };
            let ctl = r.bytes()?;
            let code = r.bytes()?;
            uniq.push((
                key,
                Shader {
                    label,
                    st,
                    ctl,
                    code,
                },
            ));
        }
        let np = r.u32()? as usize;
        let mut pairs = Vec::new();
        for _ in 0..np {
            pairs.push(Pair {
                a: r.str()?,
                b: r.str()?,
                prog: r.u8()? != 0,
            });
        }
        let extra = match r.u8()? {
            0 => {
                let nt = r.u32()? as usize;
                let mut t = Trees::new();
                for _ in 0..nt {
                    let cid = r.u32()? as i32;
                    let h: [u8; 16] = r.fixed::<16>()?;
                    let root = if r.u8()? == 1 {
                        Some(Arc::new(read_node(&mut r, 0)?))
                    } else {
                        None
                    };
                    t.insert((cid, h), root);
                }
                Extra::Trees(t)
            }
            1 => Extra::Defer,
            2 => Extra::Unreadable {
                why: r.str()?,
                count: r.u32()? as usize,
            },
            _ => return None,
        };
        out.push(FileResult {
            rel,
            uniq,
            pairs,
            extra,
        });
    }
    // the file ends here: no byte left over
    if r.left != 0 {
        return None;
    }
    Some(out)
}

pub fn decode(raw: &[u8]) -> Option<Vec<FileResult>> {
    decode_from(raw, raw.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ibuilder-shadersplit-atomic-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn no_part(d: &Path) -> bool {
        fs::read_dir(d)
            .unwrap()
            .all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".part"))
    }

    #[test]
    fn a_file_is_replaced_whole_or_not_at_all() {
        let d = dir("whole");
        let f = d.join("t.tsv");
        write_atomic(&f, |w| w.write_all(b"first\n")).unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"first\n");
        assert!(no_part(&d));
        // a fill that fails half way (a full drive): the old bytes stay, the `.part` is gone
        let e = write_atomic(&f, |w| {
            w.write_all(&[7u8; 100_000])?; // more than the BufWriter holds: some of it reached the `.part` file
            Err(std::io::Error::other("disk full"))
        });
        assert_eq!(e.unwrap_err().to_string(), "disk full");
        assert_eq!(fs::read(&f).unwrap(), b"first\n");
        assert!(no_part(&d));
        // a success replaces it
        write_atomic(&f, |w| w.write_all(b"second\n")).unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"second\n");
        assert!(no_part(&d));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_directory_in_the_way_is_an_error_and_leaves_nothing() {
        let d = dir("dir");
        // the final name is a directory: the rename cannot replace it, and the `.part` written to is removed
        fs::create_dir(d.join("t.tsv")).unwrap();
        assert!(write_atomic(&d.join("t.tsv"), |w| w.write_all(b"x")).is_err());
        assert!(d.join("t.tsv").is_dir() && no_part(&d));
        // the `.part` name is a directory: nothing can be written, the old file stays, the directory is not touched
        fs::write(d.join("u.tsv"), b"old").unwrap();
        fs::create_dir(d.join("u.tsv.part")).unwrap();
        assert!(write_atomic(&d.join("u.tsv"), |w| w.write_all(b"new")).is_err());
        assert_eq!(fs::read(d.join("u.tsv")).unwrap(), b"old");
        assert!(d.join("u.tsv.part").is_dir());
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_folder_keeps_the_old_file() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("ro");
        fs::write(d.join("t.tsv"), b"old").unwrap();
        fs::set_permissions(&d, fs::Permissions::from_mode(0o555)).unwrap();
        let r = write_atomic(&d.join("t.tsv"), |w| w.write_all(b"new"));
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
        // a root can write there whatever the mode: then it is a plain success and there is nothing to check
        if r.is_err() {
            assert_eq!(fs::read(d.join("t.tsv")).unwrap(), b"old");
        }
        assert!(no_part(&d));
        let _ = fs::remove_dir_all(&d);
    }
}
