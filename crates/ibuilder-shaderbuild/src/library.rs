//! The library image (`DMROMFS1`: `dm/tools/shader_lib.write`, `romfs_image.py`) written as a stream: each file's bytes are copied from where they are (a cache entry, a stage entry, a
//! small in-memory table) into the output in name order, so the 1.7 GB image of Grizzy is never in memory (the first port's `romfs::write_image_file` takes every file's bytes in a
//! `Vec` first: 1.9 GB of footprint). The layout is the Python's byte for byte: a 32-byte header written last, every file's data at a 16-byte aligned offset in bytes-sorted name
//! order, the 24-byte entries (a root first), the name pool.
use ibuilder_shader::romfs;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

/// Where a library file's bytes come from
#[derive(Clone, Debug)]
pub enum Source {
    Range { path: PathBuf, off: u64, len: u64 },
    Bytes(Vec<u8>),
}

impl Source {
    pub fn len(&self) -> u64 {
        match self {
            Source::Range { len, .. } => *len,
            Source::Bytes(b) => b.len() as u64,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

const HEAD: u64 = 32;
const COPY: usize = 8 << 20;

/// Writes the image (`<dst>.part`, then a rename: an image is whole or absent). Returns `(size, file count)`.
pub fn write_image(mut files: Vec<(String, Source)>, dst: &Path) -> std::io::Result<(u64, usize)> {
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    if let Some(w) = files.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(std::io::Error::other(format!(
            "two library files are named {}",
            w[0].0
        )));
    }
    let part = dst.with_extension("img.part");
    let res = (|| -> std::io::Result<(u64, usize)> {
        let f = File::create(&part)?;
        let mut w = std::io::BufWriter::with_capacity(COPY, f);
        let mut off = HEAD;
        w.write_all(&[0u8; HEAD as usize])?;
        let mut ents: Vec<(u64, u64)> = Vec::with_capacity(files.len());
        let mut buf = vec![0u8; COPY];
        let mut open: Option<(PathBuf, File)> = None;
        for (name, src) in &files {
            let pad = (romfs::ALIGN - off % romfs::ALIGN) % romfs::ALIGN;
            if pad > 0 {
                w.write_all(&vec![0u8; pad as usize])?;
                off += pad;
            }
            match src {
                Source::Bytes(b) => w.write_all(b)?,
                Source::Range { path, off: at, len } => {
                    if open.as_ref().map(|o| &o.0 != path).unwrap_or(true) {
                        open = Some((
                            path.clone(),
                            File::open(path).map_err(|e| {
                                std::io::Error::new(
                                    e.kind(),
                                    format!("{}: {e} (the file of {name})", path.display()),
                                )
                            })?,
                        ));
                    }
                    let fh = &open.as_ref().unwrap().1;
                    let mut done = 0u64;
                    while done < *len {
                        let n = ((*len - done) as usize).min(COPY);
                        fh.read_exact_at(&mut buf[..n], at + done).map_err(|e| {
                            std::io::Error::new(
                                e.kind(),
                                format!("{}: {e} (the file of {name})", path.display()),
                            )
                        })?;
                        w.write_all(&buf[..n])?;
                        done += n as u64;
                    }
                }
            }
            ents.push((off, src.len()));
            off += src.len();
        }
        let mut pool: Vec<u8> = Vec::new();
        let mut idx: Vec<u8> = Vec::with_capacity((files.len() + 1) * 24);
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
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&romfs::header(
            off,
            (idx.len() + pool.len()) as u64,
            files.len() as u64 + 1,
        ))?;
        f.flush()?;
        Ok((off + (idx.len() + pool.len()) as u64, files.len()))
    })();
    match res {
        Ok(r) => {
            std::fs::rename(&part, dst)?;
            Ok(r)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_image_equals_the_in_memory_writer_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("ibsb-lib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let blob = dir.join("blob");
        let data: Vec<u8> = (0..5000u32).map(|i| (i * 7) as u8).collect();
        std::fs::write(&blob, &data).unwrap();
        let names = [
            "b.json",
            "a_b.metal",
            "c1_full.tsv",
            "0123456789abcdef_b.metal",
            "z",
        ];
        let sources: Vec<(String, Source)> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                (
                    n.to_string(),
                    if i % 2 == 0 {
                        Source::Range {
                            path: blob.clone(),
                            off: i as u64 * 100,
                            len: 1000 + i as u64,
                        }
                    } else {
                        Source::Bytes(vec![i as u8; 33 + i])
                    },
                )
            })
            .collect();
        let want: Vec<(String, Vec<u8>)> = sources
            .iter()
            .map(|(n, s)| {
                (
                    n.clone(),
                    match s {
                        Source::Bytes(b) => b.clone(),
                        Source::Range { off, len, .. } => {
                            data[*off as usize..(*off + *len) as usize].to_vec()
                        }
                    },
                )
            })
            .collect();
        let a = dir.join("a.img");
        let b = dir.join("b.img");
        let (size, n) = write_image(sources, &a).unwrap();
        romfs::write_image_file(want.clone(), &b).unwrap();
        let (ba, bb) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        assert_eq!(ba, bb);
        assert_eq!((size, n), (ba.len() as u64, 5));
        let img = romfs::Image::open(&ba).unwrap();
        for (name, bytes) in &want {
            assert_eq!(img.bytes(img.find(name).unwrap()), &bytes[..]);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
