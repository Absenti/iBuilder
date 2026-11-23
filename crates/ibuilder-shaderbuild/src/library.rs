//! The library image (`DMROMFS1`: `dm/tools/shader_lib.write`, `romfs_image.py`) written as a stream: each file's bytes are copied from where they are (a cache entry, a stage entry, a
//! small in-memory table) into the output in name order, so the 1.7 GB image of Grizzy is never in memory (the first port's `romfs::write_image_file` takes every file's bytes in a
//! `Vec` first: 1.9 GB of footprint). The layout is the Python's byte for byte: a 32-byte header written last, every file's data at a 16-byte aligned offset in bytes-sorted name
//! order, the 24-byte entries (a root first), the name pool.
//!
//! **The digests table** (speed-up B6): beside the image, `<image>.digests` says which files of the image each finished shader's cache entry gave, under its digest, with the facts of
//! the entry (`cache::Meta`): a rebuild keeps the old image aside and serves an unchanged shader from it ([`Previous`], `cache::Store::lookup`) instead of translating it again. It is text,
//! one line per shader, `<digest> TAB <meta json> TAB (<name> TAB <offset> TAB <length>)...`, under a first line `DMSBLIB1 TAB <image size> TAB <image mtime, ns> TAB <sha256 of the
//! image's index and name pool>` that ties it to its image: a table whose image was replaced or rewritten is not believed (the index alone would not tell an image of the same names and
//! sizes; the time does: a rename or a hard link, which is all the pipeline does to a library, keeps it, a write changes it). It is written after the image is in place and never inside it (the image is the
//! same bytes with or without it). Only a source whose file is a cache entry named by its digest (or an old image's file its table names) gets a line: a shader the cache must not take
//! (`Store::uncached_stage_path`) gets none, so the next build makes it again.
use crate::cache::{Meta, read_header, write_atomic};
use ibuilder_shader::romfs;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

/// The first field of a digests table
pub const DIGESTS_MAGIC: &str = "DMSBLIB1";

/// `<image>.digests`
pub fn digests_path(image: &Path) -> PathBuf {
    let mut p = image.as_os_str().to_owned();
    p.push(".digests");
    PathBuf::from(p)
}

fn mtime_ns(m: &std::fs::Metadata) -> u128 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

/// `(image size, its modification time in ns, offset of the index)`, and the sha256 of the index and name pool (the image's tail): what a digests table is tied to
fn image_identity(path: &Path) -> std::io::Result<(u64, u128, u64, String)> {
    let f = File::open(path)?;
    let meta = f.metadata()?;
    let size = meta.len();
    let mut head = [0u8; HEAD as usize];
    f.read_exact_at(&mut head, 0)?;
    if &head[..8] != romfs::MAGIC {
        return Err(std::io::Error::other("not a DMROMFS1 image"));
    }
    let ioff = u64::from_le_bytes(head[8..16].try_into().unwrap());
    let isz = u64::from_le_bytes(head[16..24].try_into().unwrap());
    if ioff < HEAD || ioff.checked_add(isz) != Some(size) || isz > 1 << 30 {
        return Err(std::io::Error::other("the image's index is not its tail"));
    }
    let mut tail = vec![0u8; isz as usize];
    f.read_exact_at(&mut tail, ioff)?;
    Ok((
        size,
        mtime_ns(&meta),
        ioff,
        crate::key::hex_of(&Sha256::digest(&tail)),
    ))
}

fn is_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// One line of a digests table: the meta json and the files (name, offset, length)
type Line<'a> = (&'a str, Vec<(&'a str, u64, u64)>);
/// A file name of an old image -> its shader's digest, meta json and file names
type ByName<'a> = HashMap<&'a str, (&'a str, &'a str, Vec<&'a str>)>;
/// digest -> (meta json, the file names its entry has, the files placed in the new image: name, offset, length)
type Placed = BTreeMap<String, (String, Vec<String>, Vec<(String, u64, u64)>)>;

fn parse_line(line: &str) -> Option<(&str, Line<'_>)> {
    let mut f = line.split('\t');
    let digest = f.next().filter(|d| is_digest(d))?;
    let meta = f.next()?;
    let rest: Vec<&str> = f.collect();
    if rest.is_empty() || !rest.len().is_multiple_of(3) {
        return None;
    }
    let mut files = Vec::with_capacity(rest.len() / 3);
    for c in rest.chunks(3) {
        files.push((c[0], c[1].parse().ok()?, c[2].parse().ok()?));
    }
    Some((digest, (meta, files)))
}

/// A previous library and its digests table, checked against each other: every shader of the table can be served from the image (`lookup`).
pub struct Previous {
    pub image: PathBuf,
    text: String,
    /// digest -> its line's range in `text` (the lines are parsed again when a shader is looked up: 22,000 parsed entries would be ~15 MB held for the whole build)
    lines: HashMap<String, std::ops::Range<usize>>,
}

impl Previous {
    /// The image at `image` and its table `<image>.digests`; `Err` says why they cannot be used (no table, another image, a damaged line)
    pub fn open(image: &Path) -> Result<Previous, String> {
        let text = std::fs::read_to_string(digests_path(image))
            .map_err(|e| format!("no digests table ({e})"))?;
        let (size, mtime, ioff, tail) =
            image_identity(image).map_err(|e| format!("{}: {e}", image.display()))?;
        let mut it = text.split_inclusive('\n');
        let first = it.next().unwrap_or("");
        let head = first.trim_end();
        if head != format!("{DIGESTS_MAGIC}\t{size}\t{mtime}\t{tail}") {
            return Err("the digests table is of another image".into());
        }
        let mut lines = HashMap::new();
        let mut at = first.len();
        for raw in it {
            let line = raw.trim_end_matches('\n');
            let (digest, (meta, files)) =
                parse_line(line).ok_or_else(|| format!("a damaged line at byte {at}"))?;
            if Meta::from_json(meta.as_bytes()).is_none()
                || files
                    .iter()
                    .any(|(_, o, l)| *o < HEAD || o.saturating_add(*l) > ioff)
            {
                return Err(format!("a damaged line at byte {at}"));
            }
            lines.insert(digest.to_string(), at..at + line.len());
            at += raw.len();
        }
        Ok(Previous {
            image: image.to_path_buf(),
            text,
            lines,
        })
    }

    /// the shaders the table holds
    pub fn len(&self) -> usize {
        self.lines.len()
    }
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// A shader of the old image as a cache entry: its files are the old image's ranges
    pub fn lookup(&self, digest: &str) -> Option<crate::cache::Stored> {
        let r = self.lines.get(digest)?;
        let (_, (meta, files)) = parse_line(&self.text[r.clone()])?;
        Some(crate::cache::Stored {
            path: self.image.clone(),
            meta: Meta::from_json(meta.as_bytes())?,
            files: files
                .into_iter()
                .map(|(n, o, l)| (n.to_string(), o, l))
                .collect(),
        })
    }

    /// `name` -> (digest, meta json, the shader's file names): what `write_image` needs to give a file of this image its line in the new table
    fn by_name(&self) -> ByName<'_> {
        let mut m = HashMap::new();
        for r in self.lines.values() {
            if let Some((d, (meta, files))) = parse_line(&self.text[r.clone()]) {
                let names: Vec<&str> = files.iter().map(|f| f.0).collect();
                for n in &names {
                    m.insert(*n, (d, meta, names.clone()));
                }
            }
        }
        m
    }
}

/// Where a source file of the image came from, for the digests table
enum Origin {
    /// a cache entry (in the stage folder or the cache) named by its digest
    Entry {
        digest: String,
        meta: String,
        names: Vec<String>,
    },
    /// an earlier image with a table that matches it
    Library(Previous),
    None,
}

fn origin_of(path: &Path) -> Origin {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if is_digest(&name) {
        if let Some(st) = read_header(path) {
            return Origin::Entry {
                digest: name,
                meta: String::from_utf8_lossy(&st.meta.to_json()).into_owned(),
                names: st.files.into_iter().map(|f| f.0).collect(),
            };
        }
        return Origin::None;
    }
    if digests_path(path).is_file()
        && let Ok(p) = Previous::open(path)
    {
        return Origin::Library(p);
    }
    Origin::None
}

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

/// Writes the image (`<dst>.part`, then a rename: an image is whole or absent). Returns `(size, file count)`. A failed write, or a failed rename, removes its `.part`, so a
/// full drive gets its space back (nxnative e3f2d21f, `shader_lib.write`); only a killed one leaves the `.part`.
pub fn write_image(mut files: Vec<(String, Source)>, dst: &Path) -> std::io::Result<(u64, usize)> {
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    if let Some(w) = files.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(std::io::Error::other(format!(
            "two library files are named {}",
            w[0].0
        )));
    }
    // which shader each file belongs to (read before anything is written: an old image at `dst` itself is read as it was)
    let mut origins: HashMap<PathBuf, Origin> = HashMap::new();
    for (_, src) in &files {
        if let Source::Range { path, .. } = src
            && !origins.contains_key(path)
        {
            origins.insert(path.clone(), origin_of(path));
        }
    }
    let by_name: HashMap<PathBuf, ByName<'_>> = origins
        .iter()
        .filter_map(|(p, o)| match o {
            Origin::Library(prev) => Some((p.clone(), prev.by_name())),
            _ => None,
        })
        .collect();
    let table = digests_path(dst);
    let _ = std::fs::remove_file(&table); // a table never outlives the image it was made for
    let mut placed: Placed = BTreeMap::new();
    let mut tail_sha = String::new();
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
            if let Source::Range { path, .. } = src {
                let who: Option<(String, String, Vec<String>)> = match origins.get(path) {
                    Some(Origin::Entry {
                        digest,
                        meta,
                        names,
                    }) => Some((digest.clone(), meta.clone(), names.clone())),
                    Some(Origin::Library(_)) => by_name
                        .get(path)
                        .and_then(|m| m.get(name.as_str()))
                        .map(|(d, meta, names)| {
                            (
                                d.to_string(),
                                meta.to_string(),
                                names.iter().map(|n| n.to_string()).collect(),
                            )
                        }),
                    _ => None,
                };
                if let Some((d, meta, names)) = who {
                    placed
                        .entry(d)
                        .or_insert_with(|| (meta, names, Vec::new()))
                        .2
                        .push((name.clone(), off, src.len()));
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
        let mut h = Sha256::new();
        h.update(&idx);
        h.update(&pool);
        tail_sha = crate::key::hex_of(&h.finalize());
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
            std::fs::rename(&part, dst).inspect_err(|_| {
                let _ = std::fs::remove_file(&part);
            })?;
            write_digests(&table, dst, &tail_sha, placed);
            Ok(r)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// The digests table of an image just written: a line for every shader whose files are ALL in the image (a name with a tab or a line break in it cannot be written, and its shader
/// gets no line). A table that cannot be written is left out (the next build translates those shaders, as before the table existed).
fn write_digests(table: &Path, image: &Path, tail_sha: &str, placed: Placed) {
    let Ok(m) = std::fs::metadata(image) else {
        return;
    };
    let mut text = format!(
        "{DIGESTS_MAGIC}\t{}\t{}\t{tail_sha}\n",
        m.len(),
        mtime_ns(&m)
    );
    let mut n = 0usize;
    for (digest, (meta, names, mut got)) in placed {
        let mut want = names.clone();
        want.sort();
        got.sort();
        let have: Vec<&String> = got.iter().map(|g| &g.0).collect();
        if have != want.iter().collect::<Vec<_>>()
            || got.iter().any(|g| g.0.contains(['\t', '\n', '\r']))
            || meta.contains(['\t', '\n', '\r'])
        {
            continue;
        }
        text.push_str(&digest);
        text.push('\t');
        text.push_str(&meta);
        for (name, off, len) in &got {
            text.push_str(&format!("\t{name}\t{off}\t{len}"));
        }
        text.push('\n');
        n += 1;
    }
    if n > 0 && write_atomic(table, text.as_bytes()).is_err() {
        let _ = std::fs::remove_file(table);
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

    #[test]
    fn a_failed_write_or_rename_leaves_no_part() {
        // nxnative e3f2d21f: a write that fails (a full drive) gives its space back. A rename onto a non-empty folder fails after the whole .part was written
        let dir = std::env::temp_dir().join(format!("ibsb-part-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dst = dir.join("b-x.img");
        std::fs::create_dir_all(dst.join("occupied")).unwrap();
        let e = write_image(vec![("a".into(), Source::Bytes(vec![1; 100]))], &dst).unwrap_err();
        assert!(
            !dir.join("b-x.img.part").exists(),
            "the .part of a failed rename stays: {e}"
        );
        // a source that cannot be read: the copy stops halfway
        let gone = Source::Range {
            path: dir.join("missing"),
            off: 0,
            len: 10,
        };
        let dst2 = dir.join("c.img");
        write_image(
            vec![
                ("a".into(), Source::Bytes(vec![2; 4096])),
                ("b".into(), gone),
            ],
            &dst2,
        )
        .unwrap_err();
        assert!(!dir.join("c.img.part").exists() && !dst2.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
