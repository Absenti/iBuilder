//! `mvgl.py`: Media.Vision's MVGL archives (`MDB1`, little-endian, no encryption on the Switch), read in place through a [`Source`].
//!
//! * Legacy layout (Digimon Story Cyber Sleuth: Complete Edition): a 0x14-byte header (`MDB1`, u16 tree entries, u16 name entries, u32 data entries, u32 data start,
//!   u32 archive size), 8-byte tree entries, 0x40-byte name entries, 12-byte data entries; a stored length below the length is doboz-compressed (version 0).
//! * Wide layout (Digimon Story Time Stranger): a 0x20-byte header (u32 counts, u64 data start and size), 16-byte tree entries, 0x80-byte names, 16-byte data entries;
//!   a stored length below the length is a raw LZ4 block.
//!
//! The layout is told by the header's own data start (header + tables) and archive size, as the Python does. Errors carry the Python's `ValueError` texts;
//! [`MvglError::value`] says whether the Python raises `ValueError` (the GL port skips such an archive when opening it) or something else (an `IndexError` on a
//! damaged doboz stream, an I/O error).

use ibuilder_shadersplit::source::Source;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MvglError {
    /// the Python raises `ValueError` (else: `IndexError`, `OSError`)
    pub value: bool,
    pub msg: String,
}

impl std::fmt::Display for MvglError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

fn verr<T>(msg: impl Into<String>) -> Result<T, MvglError> {
    Err(MvglError {
        value: true,
        msg: msg.into(),
    })
}

fn other(msg: impl Into<String>) -> MvglError {
    MvglError {
        value: false,
        msg: msg.into(),
    }
}

/// mask, offset shift, length mask, length shift, size
const MATCH: [(u64, u32, u64, u32, usize); 8] = [
    (0xff, 2, 0, 0, 1),
    (0xffff, 2, 0, 0, 2),
    (0xffff, 6, 15, 2, 2),
    (0xffffff, 8, 31, 3, 3),
    (0xff, 2, 0, 0, 1),
    (0xffff, 2, 0, 0, 2),
    (0xffff, 6, 15, 2, 2),
    (0xffffffff, 11, 255, 3, 4),
];
const TAIL: u64 = 8;

/// `int.from_bytes(src[i:i + 4], 'little')`: fewer bytes past the end, as a slice gives them
fn le4(src: &[u8], i: usize) -> u64 {
    let mut v = 0u64;
    for k in 0..4 {
        if let Some(&b) = src.get(i + k) {
            v |= (b as u64) << (8 * k);
        }
    }
    v
}

fn le_n(b: &[u8]) -> u128 {
    b.iter().rev().fold(0u128, |acc, &x| (acc << 8) | x as u128)
}

/// `doboz(b)`: Attila T. Afra's doboz, version 0
pub fn doboz(b: &[u8]) -> Result<Vec<u8>, MvglError> {
    if b.is_empty() {
        return verr("empty doboz stream");
    }
    let a = b[0];
    if a & 7 != 0 {
        return verr(format!("doboz version {}", a & 7));
    }
    let w = (((a >> 3) & 7) + 1) as usize;
    if b.len() < 1 + 2 * w {
        return verr("truncated doboz header");
    }
    let size = le_n(&b[1..1 + w]);
    let csize = le_n(&b[1 + w..1 + 2 * w]);
    let mut i = 1 + 2 * w;
    if a & 0x80 != 0 {
        if (b.len() as u128) < i as u128 + size {
            return verr("truncated stored doboz payload");
        }
        return Ok(b[i..i + size as usize].to_vec());
    }
    let mut src = b.to_vec();
    src.extend_from_slice(&[0u8; 8]);
    let idx = |i: usize| -> Result<u8, MvglError> {
        src.get(i)
            .copied()
            .ok_or_else(|| other("index out of range"))
    };
    let mut out: Vec<u8> = Vec::new();
    let mut ctl: u64 = 1;
    while (out.len() as u128) < size {
        if ctl == 1 {
            ctl = le4(&src, i);
            i += 4;
        }
        if ctl & 1 == 0 {
            if (out.len() as i128) < size as i128 - TAIL as i128 {
                out.push(idx(i)?);
                i += 1;
            } else {
                while (out.len() as u128) < size {
                    if ctl == 1 {
                        ctl = le4(&src, i);
                        i += 4;
                    }
                    out.push(idx(i)?);
                    i += 1;
                    ctl >>= 1;
                }
                break;
            }
        } else {
            let word = le4(&src, i);
            let m = MATCH[(word & 7) as usize];
            let off = ((word & m.0) >> m.1) as usize;
            let n = (((word >> m.3) & m.2) + 3) as usize;
            i += m.4;
            if off == 0 || off > out.len() {
                return verr("doboz match out of range");
            }
            let s = out.len() - off;
            if off >= n {
                out.extend_from_within(s..s + n);
            } else {
                for k in 0..n {
                    let c = out[s + k];
                    out.push(c);
                }
            }
        }
        ctl >>= 1;
    }
    if (i - (1 + 2 * w)) as u128 > csize + 4 {
        return verr("doboz stream overrun");
    }
    Ok(out)
}

/// `lz4_block(b, size)`
pub fn lz4_block(b: &[u8], size: u64) -> Result<Vec<u8>, MvglError> {
    if size != 0 && (b.is_empty() || size > 255 * b.len() as u64) {
        return verr(format!(
            "impossible LZ4 length {size:#x} for {:#x} bytes",
            b.len()
        ));
    }
    let out = lz4_flex::block::decompress(b, size as usize).map_err(|e| MvglError {
        value: true,
        msg: format!("invalid LZ4 block: {e}"),
    })?;
    if out.len() as u64 != size {
        return verr(format!(
            "LZ4 decoded {:#x} bytes, expected {size:#x}",
            out.len()
        ));
    }
    Ok(out)
}

/// An open archive: `files` maps `name.ext` to (offset in the file, length, stored length)
pub struct Archive {
    src: Box<dyn Source>,
    pub wide: bool,
    pub start: u64,
    pub size: u64,
    pub files: BTreeMap<String, (u64, u64, u64)>,
}

fn u16a(b: &[u8], o: usize) -> u64 {
    u16::from_le_bytes([b[o], b[o + 1]]) as u64
}
fn u32a(b: &[u8], o: usize) -> u64 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as u64
}
fn u64a(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl Archive {
    /// `Archive(path)`; `path` only goes into the messages
    pub fn open(src: Box<dyn Source>, path: &str) -> Result<Archive, MvglError> {
        let io = |e: std::io::Error| other(format!("{path}: {e}"));
        let fsize = src.len() as u128;
        let head = src.read(0, 0x20).map_err(io)?;
        if head.len() < 0x14 || &head[..4] != b"MDB1" {
            return verr(format!("{path}: not an MVGL archive"));
        }
        let mut wide = None;
        let (mut nt, mut nn, mut nd, mut start, mut size) = (0u64, 0u64, 0u64, 0u64, 0u64);
        if head.len() >= 0x20 {
            let (t, n, d, s, z) = (
                u32a(&head, 4),
                u32a(&head, 8),
                u32a(&head, 12),
                u64a(&head, 16),
                u64a(&head, 24),
            );
            if 0x20u128 + t as u128 * 16 + n as u128 * 0x80 + d as u128 * 16 == s as u128
                && s <= z
                && z as u128 <= fsize
            {
                wide = Some(true);
                (nt, nn, nd, start, size) = (t, n, d, s, z);
            }
        }
        if wide.is_none() {
            let (t, n, d, s, z) = (
                u16a(&head, 4),
                u16a(&head, 6),
                u32a(&head, 8),
                u32a(&head, 12),
                u32a(&head, 16),
            );
            if 0x14 + t * 8 + n * 0x40 + d * 12 == s && s <= z && z as u128 <= fsize {
                wide = Some(false);
                (nt, nn, nd, start, size) = (t, n, d, s, z);
            }
        }
        let Some(wide) = wide else {
            return verr(format!("{path}: unrecognized MVGL header layout"));
        };
        let (hdr_sz, tree_sz, name_sz, data_sz) = if wide {
            (0x20u64, 16u64, 0x80u64, 16u64)
        } else {
            (0x14, 8, 0x40, 12)
        };
        let rd = |off: u64, n: u64| src.read(off, n as usize).map_err(io);
        let tree = rd(hdr_sz, nt * tree_sz)?;
        let names = rd(hdr_sz + nt * tree_sz, nn * name_sz)?;
        let data = rd(hdr_sz + nt * tree_sz + nn * name_sz, nd * data_sz)?;
        if tree.len() as u64 != nt * tree_sz
            || names.len() as u64 != nn * name_sz
            || data.len() as u64 != nd * data_sz
        {
            return verr(format!("{path}: truncated MVGL tables"));
        }
        let mut files = BTreeMap::new();
        for k in 0..nt {
            let at = (k * tree_sz) as usize;
            let (idx, no_idx) = if wide {
                (u32a(&tree, at + 4), 0xffff_ffffu64)
            } else {
                (u16a(&tree, at + 2), 0xffff)
            };
            if idx == no_idx {
                continue;
            }
            if k >= nn || idx >= nd {
                return verr(format!("{path}: MVGL table index out of range"));
            }
            let d = (idx * data_sz) as usize;
            let (off, n, cn) = if wide {
                (u64a(&data, d), u32a(&data, d + 8), u32a(&data, d + 12))
            } else {
                (u32a(&data, d), u32a(&data, d + 4), u32a(&data, d + 8))
            };
            if cn > n || off as u128 + cn as u128 > (size - start) as u128 {
                return verr(format!(
                    "{path}: MVGL entry {idx} lies outside archive data"
                ));
            }
            let base = (k * name_sz) as usize;
            let mut e = &names[base..base + 4];
            while let Some((&c, rest)) = e.split_last() {
                if c == 0 || c == b' ' {
                    e = rest;
                } else {
                    break;
                }
            }
            let nm = &names[base + 4..base + name_sz as usize];
            let nm = match nm.iter().position(|&c| c == 0) {
                Some(z) => &nm[..z],
                None => nm,
            };
            files.insert(
                format!("{}.{}", crate::py::latin1(nm), crate::py::latin1(e)),
                (start + off, n, cn),
            );
        }
        Ok(Archive {
            src,
            wide,
            start,
            size,
            files,
        })
    }

    /// `read(name)`: the entry's bytes, decompressed (a name the archive does not hold: an error, the Python's `KeyError`)
    pub fn read(&self, name: &str) -> Result<Vec<u8>, MvglError> {
        let &(off, n, cn) = self
            .files
            .get(name)
            .ok_or_else(|| other(format!("KeyError: {name:?}")))?;
        let b = self
            .src
            .read(off, cn as usize)
            .map_err(|e| other(e.to_string()))?;
        if b.len() as u64 != cn {
            return verr(format!(
                "truncated MVGL entry ({:#x} of {cn:#x} bytes)",
                b.len()
            ));
        }
        if cn == n {
            return Ok(b);
        }
        if cn > n {
            return verr(format!(
                "MVGL entry stored length {cn:#x} exceeds length {n:#x}"
            ));
        }
        if self.wide {
            lz4_block(&b, n)
        } else {
            doboz(&b)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibuilder_shadersplit::source::Source;

    struct Mem(Vec<u8>);
    impl Source for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, off: u64, buf: &mut [u8]) -> std::io::Result<usize> {
            let off = off as usize;
            if off >= self.0.len() {
                return Ok(0);
            }
            let n = buf.len().min(self.0.len() - off);
            buf[..n].copy_from_slice(&self.0[off..off + n]);
            Ok(n)
        }
    }

    /// a doboz stream of only literals: 1-byte widths, then control words of zeros
    fn doboz_literals(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8, data.len() as u8, 0];
        for (k, &c) in data.iter().enumerate() {
            if k % 31 == 0 {
                out.extend_from_slice(&0x8000_0000u32.to_le_bytes());
            }
            out.push(c);
        }
        let cs = (out.len() - 3) as u8;
        out[2] = cs;
        out
    }

    #[test]
    fn doboz_of_literals() {
        let d = b"abcdefghijklmnopqrstuvwxyz0123456789";
        assert_eq!(doboz(&doboz_literals(d)).unwrap(), d);
    }

    #[test]
    fn legacy_archive_lists_and_reads() {
        // one tree root (no data) + one leaf, two names, one data entry
        let mut a = Vec::new();
        let nt = 2u16;
        let nn = 2u16;
        let nd = 1u32;
        let start = 0x14 + 2 * 8 + 2 * 0x40 + 12;
        let body = b"void main(){}";
        let size = start + body.len() as u32;
        a.extend_from_slice(b"MDB1");
        a.extend_from_slice(&nt.to_le_bytes());
        a.extend_from_slice(&nn.to_le_bytes());
        a.extend_from_slice(&nd.to_le_bytes());
        a.extend_from_slice(&start.to_le_bytes());
        a.extend_from_slice(&size.to_le_bytes());
        for (bit, idx) in [(0xffffu16, 0xffffu16), (0, 0)] {
            a.extend_from_slice(&bit.to_le_bytes());
            a.extend_from_slice(&idx.to_le_bytes());
            a.extend_from_slice(&[0u8; 4]);
        }
        a.extend_from_slice(&[0u8; 0x40]);
        let mut nm = b"shad".to_vec();
        nm.extend_from_slice(b"sh\\m_vp");
        nm.resize(0x40, 0);
        a.extend_from_slice(&nm);
        a.extend_from_slice(&0u32.to_le_bytes());
        a.extend_from_slice(&(body.len() as u32).to_le_bytes());
        a.extend_from_slice(&(body.len() as u32).to_le_bytes());
        a.extend_from_slice(body);
        let ar = Archive::open(Box::new(Mem(a)), "x.mvgl").unwrap();
        assert!(!ar.wide);
        assert_eq!(ar.files.keys().collect::<Vec<_>>(), ["sh\\m_vp.shad"]);
        assert_eq!(ar.read("sh\\m_vp.shad").unwrap(), body);
        let afs = Archive::open(
            Box::new(Mem(b"AFS2xxxxxxxxxxxxxxxxxxxx".to_vec())),
            "s.mvgl",
        );
        assert!(matches!(afs, Err(MvglError { value: true, .. })));
    }
}
