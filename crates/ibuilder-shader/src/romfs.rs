//! The shader library image (`DMROMFS1`, dm/tools/romfs_image.py + shader_lib.py): a flat, name-sorted list of files. Reader and writer; the writer reproduces
//! `shader_lib.write` byte for byte.
use std::io::Write;
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"DMROMFS1";
const HEAD: usize = 32;
const ENT: usize = 24;
pub const ALIGN: u64 = 16;

/// a file of the image: name and its byte range
#[derive(Debug, Clone)]
pub struct FileRef {
    pub name: String,
    pub off: u64,
    pub size: u64,
}

/// An image held in memory (the caller maps or reads the bytes). Only the flat layout shader_lib.write makes is understood (root + files).
pub struct Image<'a> {
    data: &'a [u8],
    pub files: Vec<FileRef>,
}

fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

impl<'a> Image<'a> {
    pub fn open(data: &'a [u8]) -> Result<Image<'a>, String> {
        if data.len() < HEAD || &data[..8] != MAGIC {
            return Err("not a DMROMFS1 image".into());
        }
        // The header and every entry come from the file: all the arithmetic is in u128 (a u64 sum or product cannot wrap there), and a count, a range or a name that does not fit INSIDE the
        // image is an error before anything is sliced or reserved (`count * 24` wraps in u64 for a count of 2^63: a panic, in a release build an abort).
        let (ioff, isz, count) = (
            u64le(data, 8) as u128,
            u64le(data, 16) as u128,
            u64le(data, 24) as u128,
        );
        if ioff < HEAD as u128
            || ioff + isz > data.len() as u128
            || count == 0
            || count * ENT as u128 > isz
        {
            return Err("invalid RomFS image header".into());
        }
        // from here on everything is at most the image's length
        let (ioff, isz, count) = (ioff as usize, isz as usize, count as usize);
        let idx = &data[ioff..ioff + isz];
        let pool = count * ENT;
        // entry 0: the root directory (a = first child = 1, b = number of children)
        let (a0, b0) = (u64le(idx, 0), u64le(idx, 8));
        if a0 != 1 || b0 != (count - 1) as u64 {
            return Err("not a flat shader library image".into());
        }
        // a count the index can hold is still not a reason to reserve that many entries up front (they are 40 bytes each): grow as the entries prove themselves
        let mut files = Vec::with_capacity((count - 1).min(1 << 16));
        for k in 1..count {
            let o = k * ENT;
            let (a, b, no, nl, t) = (
                u64le(idx, o),
                u64le(idx, o + 8),
                u32le(idx, o + 16) as usize,
                u16::from_le_bytes([idx[o + 20], idx[o + 21]]) as usize,
                idx[o + 22],
            );
            if t != 1 || pool + no + nl > idx.len() || a as u128 + b as u128 > ioff as u128 {
                return Err(format!("invalid entry {k}"));
            }
            let name = String::from_utf8_lossy(&idx[pool + no..pool + no + nl]).into_owned();
            files.push(FileRef {
                name,
                off: a,
                size: b,
            });
        }
        Ok(Image { data, files })
    }
    pub fn bytes(&self, f: &FileRef) -> &'a [u8] {
        &self.data[f.off as usize..(f.off + f.size) as usize]
    }
    /// binary search by name (entries are sorted by bytes)
    pub fn find(&self, name: &str) -> Option<&FileRef> {
        self.files
            .binary_search_by(|f| f.name.as_bytes().cmp(name.as_bytes()))
            .ok()
            .map(|i| &self.files[i])
    }
}

/// the header the writer patches in at offset 0 (the Python seeks back and writes it last); returned separately so a streaming writer can do the same
pub fn header(index_off: u64, index_size: u64, count: u64) -> [u8; HEAD] {
    let mut h = [0u8; HEAD];
    h[..8].copy_from_slice(MAGIC);
    h[8..16].copy_from_slice(&index_off.to_le_bytes());
    h[16..24].copy_from_slice(&index_size.to_le_bytes());
    h[24..32].copy_from_slice(&count.to_le_bytes());
    h
}

/// `shader_lib.write` to a file path: whole, via `<dst>.part` (an image is whole or absent). `files`: (name, bytes) -- the bytes are written in name order
pub fn write_image_file(files: Vec<(String, Vec<u8>)>, dst: &Path) -> std::io::Result<()> {
    let mut files = files;
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let part = dst.with_extension("img.part");
    let res = (|| -> std::io::Result<()> {
        let f = std::fs::File::create(&part)?;
        let mut w = std::io::BufWriter::with_capacity(8 << 20, f);
        let mut off = HEAD as u64;
        w.write_all(&[0u8; HEAD])?;
        let mut ents = Vec::with_capacity(files.len());
        for (_, data) in &files {
            let pad = (ALIGN - off % ALIGN) % ALIGN;
            if pad > 0 {
                w.write_all(&vec![0u8; pad as usize])?;
                off += pad;
            }
            w.write_all(data)?;
            ents.push((off, data.len() as u64));
            off += data.len() as u64;
        }
        let mut pool: Vec<u8> = Vec::new();
        let mut idx: Vec<u8> = Vec::with_capacity((files.len() + 1) * ENT);
        idx.extend_from_slice(&1u64.to_le_bytes());
        idx.extend_from_slice(&(files.len() as u64).to_le_bytes());
        idx.extend_from_slice(&[0u8; 8]);
        for ((name, _), (a, b)) in files.iter().zip(&ents) {
            idx.extend_from_slice(&a.to_le_bytes());
            idx.extend_from_slice(&b.to_le_bytes());
            idx.extend_from_slice(&(pool.len() as u32).to_le_bytes());
            idx.extend_from_slice(&(name.len() as u16).to_le_bytes());
            idx.push(1);
            idx.push(0);
            pool.extend_from_slice(name.as_bytes());
        }
        w.write_all(&idx)?;
        w.write_all(&pool)?;
        let mut f = w.into_inner().map_err(|e| e.into_error())?;
        use std::io::{Seek, SeekFrom};
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header(
            off,
            (idx.len() + pool.len()) as u64,
            files.len() as u64 + 1,
        ))?;
        Ok(())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&part);
        return res;
    }
    std::fs::rename(&part, dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le(v: u64) -> [u8; 8] {
        v.to_le_bytes()
    }

    /// a well-formed image of one file, built by the writer's own layout rules
    fn honest() -> Vec<u8> {
        let mut b = header(32 + 8, 24 * 2 + 1, 2).to_vec();
        // index at 40: root (a = 1, b = 1), file entry (off 32, size 8, name at 0, len 1, type 1), pool "f"
        b.extend_from_slice(&[0u8; 8]); // the file's bytes at 32..40 (the header is 32 bytes)
        b[32..40].copy_from_slice(b"abcdefgh");
        let mut idx = Vec::new();
        idx.extend_from_slice(&le(1));
        idx.extend_from_slice(&le(1));
        idx.extend_from_slice(&[0u8; 8]);
        idx.extend_from_slice(&le(32));
        idx.extend_from_slice(&le(8));
        idx.extend_from_slice(&0u32.to_le_bytes());
        idx.extend_from_slice(&1u16.to_le_bytes());
        idx.push(1);
        idx.push(0);
        idx.push(b'f');
        b.extend_from_slice(&idx);
        b
    }

    #[test]
    fn the_honest_image_opens() {
        let b = honest();
        let img = Image::open(&b).expect("a well-formed image opens");
        assert_eq!(img.files.len(), 1);
        assert_eq!(img.bytes(img.find("f").unwrap()), b"abcdefgh");
    }

    fn with_header(ioff: u64, isz: u64, count: u64) -> Vec<u8> {
        let mut b = honest();
        b[8..16].copy_from_slice(&le(ioff));
        b[16..24].copy_from_slice(&le(isz));
        b[24..32].copy_from_slice(&le(count));
        // the root entry (at the index) says it has count - 1 children, as a writer's would: the shape check passes and only the arithmetic has to refuse it
        if ioff == 40 {
            b[48..56].copy_from_slice(&le(count.wrapping_sub(1)));
        }
        b
    }

    /// `count * 24` wrapped to 0 modulo 2^64 for count = 2^63 + k: the guard passed and `Vec::with_capacity(count - 1)` panicked with "capacity overflow" (review finding 4.1)
    #[test]
    fn a_count_whose_product_wraps_is_an_error() {
        for count in [
            1u64 << 63,
            (1u64 << 63) + 2,
            u64::MAX,
            (u64::MAX / 24) + 1,
            1 << 40,
        ] {
            let b = with_header(40, 49, count);
            assert!(Image::open(&b).is_err(), "count {count:#x}");
        }
    }

    /// `ioff + isz` wrapped past 2^64 and the slice `data[ioff..ioff + isz]` panicked
    #[test]
    fn an_index_range_that_overflows_is_an_error() {
        for (ioff, isz) in [
            (u64::MAX - 4, 16u64),
            (40, u64::MAX),
            (u64::MAX, u64::MAX),
            (1u64 << 63, 1u64 << 63),
            (40, 1 << 40),
        ] {
            let b = with_header(ioff, isz, 2);
            assert!(Image::open(&b).is_err(), "ioff {ioff:#x} isz {isz:#x}");
        }
    }

    #[test]
    fn an_entry_that_overflows_is_an_error() {
        // file entry: a (offset) near 2^64 with a size that wraps back below the index offset
        let mut b = honest();
        b[40 + 24..40 + 32].copy_from_slice(&le(u64::MAX - 3));
        b[40 + 32..40 + 40].copy_from_slice(&le(8));
        assert!(Image::open(&b).is_err());
        // a name that runs off the pool
        let mut b = honest();
        b[40 + 24 + 16..40 + 24 + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Image::open(&b).is_err());
    }

    #[test]
    fn every_truncation_and_every_overwritten_header_word_is_an_error_or_an_image_never_a_panic() {
        let good = honest();
        for len in 0..=good.len() {
            let _ = Image::open(&good[..len]);
        }
        for at in 0..good.len() {
            for v in [0x00u8, 0x7F, 0x80, 0xFF] {
                let mut b = good.clone();
                b[at] = v;
                if let Ok(img) = Image::open(&b) {
                    for f in img.files.clone() {
                        let _ = img.bytes(&f);
                    }
                }
            }
        }
    }
}
