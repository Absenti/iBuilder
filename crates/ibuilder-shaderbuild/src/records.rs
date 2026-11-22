//! The split's packages (`pkg/<stage>_<main|data>.bin`): `dmsh_build.record_index` / `rec_of`. A package is a run of records, each starting with `BTCDBTCD`
//! and a 8-byte key (`shader_split.py`); a record's bytes are from its marker to the next one. Nothing is kept in memory: the index is `key -> (start, end)`
//! and a record is read with a positioned read when a shader is built (a Pokopia package is 4.25 GB; the Python mapped it and closed the map again for the same reason).
use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MARKER: &[u8; 8] = b"BTCDBTCD";

pub struct Package {
    file: File,
    /// key (16 hex) -> (start, end); a key that occurs twice keeps the later record, as the Python's dict comprehension does
    index: HashMap<String, (u64, u64)>,
}

/// `record_index`: every marker's offset in the file (non-overlapping, as `re.finditer`), read in chunks
pub fn marker_offsets(file: &File) -> std::io::Result<(Vec<u64>, u64)> {
    let len = file.metadata()?.len();
    let finder = memchr::memmem::Finder::new(MARKER);
    let mut offs = Vec::new();
    const CHUNK: usize = 8 << 20;
    let mut buf = vec![0u8; CHUNK + MARKER.len() - 1];
    let mut pos = 0u64; // start of the window
    let mut next_ok = 0u64; // first offset a match may start at (matches do not overlap)
    while pos < len {
        let want = ((len - pos) as usize).min(buf.len());
        file.read_exact_at(&mut buf[..want], pos)?;
        let win = &buf[..want];
        let mut from = 0usize;
        while let Some(i) = finder.find(&win[from..]) {
            let at = pos + (from + i) as u64;
            if at >= next_ok {
                offs.push(at);
                next_ok = at + MARKER.len() as u64;
            }
            from += i + 1;
        }
        if pos + want as u64 >= len {
            break;
        }
        pos += CHUNK as u64; // the next window overlaps by MARKER.len() - 1 bytes, so a marker across the boundary is found once
    }
    Ok((offs, len))
}

impl Package {
    pub fn open(path: &Path) -> std::io::Result<Package> {
        let file = File::open(path)?;
        let (offs, len) = marker_offsets(&file)?;
        let mut index = HashMap::with_capacity(offs.len());
        for (i, &o) in offs.iter().enumerate() {
            let end = offs.get(i + 1).copied().unwrap_or(len);
            let mut key = [0u8; 8];
            if file.read_exact_at(&mut key, o + 8).is_err() {
                continue; // a marker at the very end of the file: no record
            }
            index.insert(hex(&key), (o, end));
        }
        Ok(Package { file, index })
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
    pub fn has(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }
    /// the sole key of a package whose records carry no keys (`'0' * 16`): the Python's hint for an old split
    pub fn all_keys_zero(&self) -> bool {
        !self.index.is_empty() && self.index.keys().all(|k| k.bytes().all(|b| b == b'0'))
    }

    pub fn read(&self, key: &str) -> Option<std::io::Result<Vec<u8>>> {
        let &(o, e) = self.index.get(key)?;
        let mut v = vec![0u8; (e - o) as usize];
        Some(self.file.read_exact_at(&mut v, o).map(|_| v))
    }
}

pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(char::from_digit((x >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((x & 15) as u32, 16).unwrap());
    }
    s
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let b = s.as_bytes();
    let d = |c: u8| (c as char).to_digit(16).map(|v| v as u8);
    (0..b.len() / 2)
        .map(|i| Some(d(b[2 * i])? << 4 | d(b[2 * i + 1])?))
        .collect()
}

/// The packages of a split folder, opened when a shader of one is first asked for (`rec_of`)
pub struct Packages {
    dir: PathBuf,
    open: Mutex<HashMap<String, std::sync::Arc<Package>>>,
}

impl Packages {
    pub fn new(dir: &Path) -> Packages {
        Packages {
            dir: dir.to_path_buf(),
            open: Mutex::new(HashMap::new()),
        }
    }

    pub fn package(&self, name: &str) -> Result<std::sync::Arc<Package>, String> {
        let mut g = self.open.lock().unwrap();
        if let Some(p) = g.get(name) {
            return Ok(p.clone());
        }
        let path = self.dir.join(name);
        let p = std::sync::Arc::new(
            Package::open(&path).map_err(|e| format!("{}: {e}", path.display()))?,
        );
        g.insert(name.to_string(), p.clone());
        Ok(p)
    }

    /// `rec_of`'s record; the error is the Python's `sys.exit` text
    pub fn record(&self, name: &str, key: &str) -> Result<Vec<u8>, String> {
        let p = self.package(name)?;
        match p.read(key) {
            Some(Ok(v)) => Ok(v),
            Some(Err(e)) => Err(format!("{name}: {e}")),
            None => {
                let old = if p.all_keys_zero() {
                    " (its records carry no keys: it was split before they did)"
                } else {
                    ""
                };
                Err(format!(
                    "[RED] {name} holds no record of shader {key}, which the shader map names: the package and the map come from different shader_split.py runs{old}; split the shaders again"
                ))
            }
        }
    }

    pub fn has(&self, name: &str, key: &str) -> Result<bool, String> {
        Ok(self.package(name)?.has(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn markers_across_chunk_boundaries() {
        // markers at 0, and 3 bytes before an 8 MiB boundary (the chunk overlap case), one at the very end with no key
        let dir = std::env::temp_dir().join(format!("ibsb-rec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.bin");
        let mut data = vec![7u8; (8 << 20) + 100];
        let put = |data: &mut Vec<u8>, at: usize, key: u8| {
            data[at..at + 8].copy_from_slice(MARKER);
            for i in 0..8 {
                data[at + 8 + i] = key;
            }
        };
        put(&mut data, 0, 0x11);
        put(&mut data, (8 << 20) - 3, 0x22);
        put(&mut data, (8 << 20) + 50, 0x33);
        File::create(&p).unwrap().write_all(&data).unwrap();
        let pk = Package::open(&p).unwrap();
        assert_eq!(pk.len(), 3);
        let r = pk.read("2222222222222222").unwrap().unwrap();
        assert_eq!(r.len(), (8 << 20) + 50 - ((8 << 20) - 3));
        assert_eq!(&r[..8], MARKER);
        let r0 = pk.read("1111111111111111").unwrap().unwrap();
        assert_eq!(r0.len(), (8 << 20) - 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(hex(&[0x0a, 0xff]), "0aff");
        assert_eq!(unhex("0aff"), Some(vec![0x0a, 0xff]));
        assert_eq!(unhex("0"), None);
    }
}
