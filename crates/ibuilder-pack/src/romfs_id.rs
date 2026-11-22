//! The RomFS identity line: FNV-1a 64 over the little-endian 64-bit words of the 32-byte header and then of the
//! index tables, plus the whole file size (`romfs_image.py identity()`, ported in `pack_nxgame.py`).
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;
const CHUNK: usize = 1 << 20; // a multiple of 8: only the final piece can hold a partial word

fn mix(mut h: u64, data: &[u8]) -> u64 {
    for w in data.chunks(8) {
        let mut b = [0u8; 8];
        b[..w.len()].copy_from_slice(w);
        h = (h ^ u64::from_le_bytes(b)).wrapping_mul(PRIME);
    }
    h
}

/// `DMROMFSID1 title=.. version=.. build=.. size=N index=<hex>`; `Ok(None)` when the file is not a complete
/// RomFS image (the Python returns `None`). Empty labels are left out.
pub fn romfs_identity_of_image(
    img: &Path,
    title: &str,
    version: &str,
    build: &str,
) -> io::Result<Option<String>> {
    let mut f = File::open(img)?;
    let size = f.metadata()?.len();
    identity_of(&mut f, size, title, version, build)
}

/// [`romfs_identity_of_image`] over any reader of `size` bytes
fn identity_of<R: Read + Seek>(
    f: &mut R,
    size: u64,
    title: &str,
    version: &str,
    build: &str,
) -> io::Result<Option<String>> {
    let mut head = [0u8; 32];
    if f.read_exact(&mut head).is_err() || &head[..8] != b"DMROMFS1" {
        return Ok(None);
    }
    let index_offset = u64::from_le_bytes(head[8..16].try_into().unwrap());
    let index_size = u64::from_le_bytes(head[16..24].try_into().unwrap());
    if index_offset < 32 || index_offset > size || index_size != size - index_offset {
        return Ok(None);
    }
    let mut h = mix(OFFSET_BASIS, &head);
    f.seek(SeekFrom::Start(index_offset))?;
    let mut left = index_size;
    let mut buf = vec![0u8; CHUNK];
    while left > 0 {
        let want = left.min(CHUNK as u64) as usize;
        // the whole chunk or nothing: `read` may return fewer bytes, and a piece that is not a multiple of 8 would shift every later word
        // (its partial word zero-padded): a wrong identity, without an error. A file that ends early is not a complete image.
        match f.read_exact(&mut buf[..want]) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        h = mix(h, &buf[..want]);
        left -= want as u64;
    }
    let mut tokens = Vec::new();
    if !title.is_empty() {
        tokens.push(format!("title={title}"));
    }
    if !version.is_empty() {
        tokens.push(format!("version={version}"));
    }
    if !build.is_empty() {
        tokens.push(format!("build={build}"));
    }
    tokens.push(format!("size={size}"));
    tokens.push(format!("index={h:x}"));
    Ok(Some(format!("DMROMFSID1 {}", tokens.join(" "))))
}

/// The `DMROMFSID1 size=<n> index=<hex>` form `package.json` carries (`romfs_identity_short`).
pub fn romfs_identity_short(line: &str) -> String {
    if line.is_empty() {
        return String::new();
    }
    // re.findall(r'(\w+)=(\S+)', line): the first value of each key wins in a dict(); later ones overwrite
    let mut fields: Vec<(String, String)> = Vec::new();
    let b = line.as_bytes();
    let is_w = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = 0;
    while i < b.len() {
        // find a run of \w+ followed by '='
        let rest = &line[i..];
        let Some(c) = rest.chars().next() else { break };
        if !is_w(c) {
            i += c.len_utf8();
            continue;
        }
        let start = i;
        let mut j = i;
        for ch in rest.chars() {
            if is_w(ch) {
                j += ch.len_utf8();
            } else {
                break;
            }
        }
        if line[j..].starts_with('=') {
            let vstart = j + 1;
            let mut k = vstart;
            for ch in line[vstart..].chars() {
                if crate::pyutil::is_py_space(ch) {
                    break;
                }
                k += ch.len_utf8();
            }
            if k > vstart {
                let key = line[start..j].to_string();
                let val = line[vstart..k].to_string();
                if let Some(slot) = fields.iter_mut().find(|(kk, _)| *kk == key) {
                    slot.1 = val;
                } else {
                    fields.push((key, val));
                }
                i = k;
                continue;
            }
        }
        i = j;
    }
    let get = |k: &str| {
        fields
            .iter()
            .find(|(kk, _)| kk == k)
            .map(|(_, v)| v.clone())
    };
    match (get("size"), get("index")) {
        (Some(s), Some(x)) => format!("DMROMFSID1 size={s} index={x}"),
        _ => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_form() {
        let long = "DMROMFSID1 title=0100DFA027504000 version=1.0.0 build=v0 size=2498033693 index=16253e096bc20e18";
        assert_eq!(
            romfs_identity_short(long),
            "DMROMFSID1 size=2498033693 index=16253e096bc20e18"
        );
        assert_eq!(romfs_identity_short(""), "");
        assert_eq!(romfs_identity_short("weird line"), "weird line");
    }
}

#[cfg(test)]
mod short_read_tests {
    use super::*;
    use std::io::Cursor;

    /// a reader that hands out at most 3 bytes per `read` (a pipe, a network file system, a signal)
    struct Trickle(Cursor<Vec<u8>>);
    impl Read for Trickle {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            let n = b.len().min(3);
            self.0.read(&mut b[..n])
        }
    }
    impl Seek for Trickle {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            self.0.seek(p)
        }
    }

    fn image(index: &[u8]) -> Vec<u8> {
        let mut v = b"DMROMFS1".to_vec();
        v.extend_from_slice(&40u64.to_le_bytes()); // index offset: after 8 data bytes
        v.extend_from_slice(&(index.len() as u64).to_le_bytes());
        v.extend_from_slice(&0u64.to_le_bytes());
        v.extend_from_slice(b"DATADATA");
        v.extend_from_slice(index);
        v
    }

    /// Review 2026-11-20 §4.12: short reads give the identity of a whole read, not one with shifted words
    #[test]
    fn short_reads_give_the_same_identity_as_whole_ones() {
        let index: Vec<u8> = (0..100u8).map(|i| i.wrapping_mul(37)).collect();
        let img = image(&index);
        let size = img.len() as u64;
        let whole = identity_of(&mut Cursor::new(img.clone()), size, "t", "", "").unwrap();
        let mut expect = mix(OFFSET_BASIS, &img[..32]);
        expect = mix(expect, &index);
        assert_eq!(
            whole,
            Some(format!("DMROMFSID1 title=t size={size} index={expect:x}"))
        );
        let trickled =
            identity_of(&mut Trickle(Cursor::new(img.clone())), size, "t", "", "").unwrap();
        assert_eq!(trickled, whole);
        // a file that ends before its index does is not an image
        let cut = img[..img.len() - 5].to_vec();
        assert_eq!(
            identity_of(&mut Trickle(Cursor::new(cut)), size, "", "", "").unwrap(),
            None
        );
    }
}
