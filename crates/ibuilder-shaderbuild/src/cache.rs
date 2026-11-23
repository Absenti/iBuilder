//! The translation cache and the staging store of one build: a content-addressed folder, one file per finished shader.
//!
//! **Why not the engine's SQLite store.** `shader_cache.py` keeps every output of every shader in one database (`cas-<chain>.db`) that its key hashes the engine's own source text into
//! (`dmsh_build.CODE_FP`), so no key written here can equal one of the Python's; the outputs are what has to be identical, and they are proven so by the tests, not the keys. This store
//! needs no SQLite, no lock and no daemon:
//!
//! * **Entry** `<cas>/cas-<chain>.dmsb/<hh>/<digest>`: a container (below) of what the shader's `finish` produced, under its final library names, plus the facts the build takes from them
//!   (the tables' rows, the stage). `<digest>` is the sha256 of everything the outputs depend on (`key::digest`). Entries are immutable: written to a temporary name beside the entry and renamed
//!   into place (atomic on every filesystem the app runs on), so a reader sees a whole entry or none, two builds that make the same entry make the same bytes, and a kill leaves only a stray
//!   `.tmp` file (removed by the next build's sweep).
//! * **Prune-safe**: a build never trusts that an entry it has seen is still there. A hit is **hard-linked into the build's own stage folder** when it is looked up (so another build or the
//!   app's cache limit deleting the entry cannot take the bytes away from this build: unlink only drops a name), and the final library is composed from the stage folder. A filesystem
//!   without hard links falls back to reading the entry in place, and a vanished entry then is the one error the build reports (rerun: the entries that remain are hits).
//! * **Concurrency-safe**: two builds (a live translation during a game, the stage 3 build) can share a cache folder: nothing is updated in place.
//! * **Other chains** (`cas-<other>.dmsb`) can never hit (the chain is in the digest): `sweep_other_chains` removes them, as the pipeline's `shader_cache_folder` does for the
//!   engine's `cas-<other>.db` files.
//!
//! * **The previous library** (speed-up B6): a rebuild of a game whose library is stale (a game update, a DLC change, a re-split) keeps the old library aside at
//!   `<out_dir>/prev/b-<chain>.img` with its digests table (`library::digests_path`). `Store::open` finds it there by convention (the stage folder is `<out_dir>/_work/stage`), and a
//!   lookup the cache misses is served from the old image (`Stored::path` is the old image, the files' ranges are inside it): an unchanged shader is not translated again, and its bytes
//!   are the ones the digest promises, so the new image is the one a build without the old library makes. The counts are left in `<out_dir>/prev/reuse.tsv` when the store goes.
//!
//! Container (all integers little endian): `"DMSBCAS1"`, `u32 meta_len`, `meta` (JSON), `u32 count`, `count` x (`u16 name_len`, `name`, `u64 data_len`), then the files' bytes in that order.
//! The header is a few hundred bytes: a lookup reads it alone and knows every file's offset and size.
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

pub const MAGIC: &[u8; 8] = b"DMSBCAS1";

/// What the build keeps of a shader besides its files: the tables' rows (computed once, from the final files) and whether they were computed
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// the `.metal` names `c1_guard` does not prove bounded
    pub c1: Vec<String>,
    /// `cb_extent`'s row `key\ttokens` for the shader's `_b.json`, if it has one
    pub cb: Option<(String, String)>,
    /// whether `c1` / `cb` were computed (a build with `tables=false` does not)
    pub tables: bool,
    /// the `user(locnN)` numbers of the shader's MSL (what `vs_pad` of a paired vertex shader reads of a pixel shader)
    pub locs: Vec<u32>,
}

impl Meta {
    pub(crate) fn to_json(&self) -> Vec<u8> {
        let cb = match &self.cb {
            Some((k, t)) => json!([k, t]),
            None => Value::Null,
        };
        json!({"tables": self.tables, "c1": self.c1, "cb": cb, "locs": self.locs})
            .to_string()
            .into_bytes()
    }
    pub(crate) fn from_json(b: &[u8]) -> Option<Meta> {
        let v: Value = serde_json::from_slice(b).ok()?;
        let c1 = v
            .get("c1")?
            .as_array()?
            .iter()
            .map(|x| x.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()?;
        let cb = match v.get("cb")? {
            Value::Null => None,
            Value::Array(a) if a.len() == 2 => {
                Some((a[0].as_str()?.to_string(), a[1].as_str()?.to_string()))
            }
            _ => return None,
        };
        let locs = v
            .get("locs")?
            .as_array()?
            .iter()
            .map(|x| x.as_u64().map(|n| n as u32))
            .collect::<Option<Vec<_>>>()?;
        Some(Meta {
            c1,
            cb,
            tables: v.get("tables")?.as_bool()?,
            locs,
        })
    }
}

/// An entry on disk: where it is and where each file lies in it
#[derive(Clone, Debug)]
pub struct Stored {
    pub path: PathBuf,
    pub meta: Meta,
    /// name, offset in the container, length
    pub files: Vec<(String, u64, u64)>,
}

impl Stored {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.2).sum()
    }

    /// one file's bytes (`None`: the name is not in the entry); an I/O error is the entry vanishing or damaged
    pub fn read(&self, name: &str) -> Option<std::io::Result<Vec<u8>>> {
        let (_, off, len) = self.files.iter().find(|f| f.0 == name)?;
        let go = || -> std::io::Result<Vec<u8>> {
            let f = File::open(&self.path)?;
            let mut v = vec![0u8; *len as usize];
            f.read_exact_at(&mut v, *off)?;
            Ok(v)
        };
        Some(go())
    }

    /// every file, in the entry's order
    pub fn read_all(&self) -> std::io::Result<Vec<(String, Vec<u8>)>> {
        let f = File::open(&self.path)?;
        let mut out = Vec::with_capacity(self.files.len());
        for (n, off, len) in &self.files {
            let mut v = vec![0u8; *len as usize];
            f.read_exact_at(&mut v, *off)?;
            out.push((n.clone(), v));
        }
        Ok(out)
    }
}

/// The container's bytes
pub fn encode(meta: &Meta, files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let m = meta.to_json();
    let mut out = Vec::with_capacity(
        32 + m.len()
            + files
                .iter()
                .map(|f| f.0.len() + 10 + f.1.len())
                .sum::<usize>(),
    );
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(m.len() as u32).to_le_bytes());
    out.extend_from_slice(&m);
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (n, d) in files {
        out.extend_from_slice(&(n.len() as u16).to_le_bytes());
        out.extend_from_slice(n.as_bytes());
        out.extend_from_slice(&(d.len() as u64).to_le_bytes());
    }
    for (_, d) in files {
        out.extend_from_slice(d);
    }
    out
}

/// Reads and checks an entry's header (`None`: absent, damaged, or shorter than its header says)
pub fn read_header(path: &Path) -> Option<Stored> {
    let file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    // the header is a few small reads: one buffered read of the file's start instead of one system call each
    let mut f = std::io::BufReader::with_capacity(4096, file);
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    if &head[..8] != MAGIC {
        return None;
    }
    let mlen = u32::from_le_bytes(head[8..12].try_into().unwrap()) as usize;
    if 12 + mlen as u64 + 4 > size || mlen > (1 << 24) {
        return None;
    }
    let mut mb = vec![0u8; mlen];
    f.read_exact(&mut mb).ok()?;
    let meta = Meta::from_json(&mb)?;
    let mut cb = [0u8; 4];
    f.read_exact(&mut cb).ok()?;
    let count = u32::from_le_bytes(cb) as usize;
    if count > 4096 {
        return None;
    }
    let mut table: Vec<(String, u64)> = Vec::with_capacity(count);
    let mut hdr = 12 + mlen as u64 + 4;
    for _ in 0..count {
        let mut nl = [0u8; 2];
        f.read_exact(&mut nl).ok()?;
        let nl = u16::from_le_bytes(nl) as usize;
        let mut nb = vec![0u8; nl];
        f.read_exact(&mut nb).ok()?;
        let mut dl = [0u8; 8];
        f.read_exact(&mut dl).ok()?;
        table.push((String::from_utf8(nb).ok()?, u64::from_le_bytes(dl)));
        hdr += 2 + nl as u64 + 8;
    }
    let mut off = hdr;
    let mut files = Vec::with_capacity(count);
    for (n, l) in table {
        files.push((n, off, l));
        off += l;
    }
    if off != size {
        return None; // truncated (or longer than it says): not an entry
    }
    Some(Stored {
        path: path.to_path_buf(),
        meta,
        files,
    })
}

fn tmp_suffix() -> String {
    format!(
        ".tmp-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    )
    .replace(['(', ')'], "")
}

/// Writes `bytes` to `path` whole (a temporary name, then a rename)
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(tmp_suffix());
    let tmp = PathBuf::from(tmp);
    let r = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.flush()
    })();
    if let Err(e) = r {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// The cache of one chain plus the build's stage folder
pub struct Store {
    /// `<cas>/cas-<chain>.dmsb`
    pub cache: PathBuf,
    /// the build's own folder of entries (hard links to cache entries, or entries that were not cacheable)
    pub stage: PathBuf,
    /// an entry the cache could not take (a full drive): only the shaders cached before that are reused by the next build
    pub cache_full: std::sync::atomic::AtomicBool,
    /// the previous library of this chain (`<out_dir>/prev`), when one is there and its digests table matches it
    prev: Option<crate::library::Previous>,
    /// `<out_dir>/prev` when a previous library was there (used or not): where the counts go when the store goes
    prev_dir: Option<PathBuf>,
    /// "used", or why the previous library was not
    prev_state: String,
    lookups: AtomicUsize,
    prev_hits: AtomicUsize,
}

/// The folder of the build's out folder that holds the previous library (`<out_dir>/prev/b-<chain>.img` and its digests table)
pub const PREV_DIR: &str = "prev";
/// What a store that found a previous library leaves in `<out_dir>/prev` when it goes: `state`, `entries`, `looked`, `hits` rows (`<name> TAB <value>`)
pub const REUSE_FILE: &str = "reuse.tsv";

impl Drop for Store {
    /// a previous library was there: what it served is left beside it for the pipeline's log (the build's report does not reach the pipeline)
    fn drop(&mut self) {
        let Some(dir) = &self.prev_dir else { return };
        let text = format!(
            "state\t{}\nentries\t{}\nlooked\t{}\nhits\t{}\n",
            self.prev_state,
            self.prev.as_ref().map_or(0, |p| p.len()),
            self.lookups.load(Ordering::Relaxed),
            self.prev_hits.load(Ordering::Relaxed)
        );
        let _ = write_atomic(&dir.join(REUSE_FILE), text.as_bytes());
    }
}

/// Whether an I/O error is a full drive (ENOSPC; EDQUOT, a quota, says the same to the player)
pub fn is_disk_full(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::StorageFull
        || e.kind() == std::io::ErrorKind::QuotaExceeded
        || matches!(e.raw_os_error(), Some(28) | Some(69))
}

impl Store {
    pub fn open(cas: &Path, chain: &str, stage: &Path) -> std::io::Result<Store> {
        let cache = cas.join(format!("cas-{chain}.dmsb"));
        fs::create_dir_all(&cache)?;
        let _ = fs::remove_dir_all(stage);
        fs::create_dir_all(stage)?;
        // by convention the stage is `<out_dir>/_work/stage`, and the previous library `<out_dir>/prev/b-<chain>.img`
        let prev_dir = stage
            .parent()
            .and_then(Path::parent)
            .map(|o| o.join(PREV_DIR));
        let prev_img = prev_dir
            .as_ref()
            .map(|d| d.join(format!("b-{chain}.img")))
            .filter(|p| p.is_file());
        let (prev, prev_state) = match &prev_img {
            None => (None, String::new()),
            Some(img) => match crate::library::Previous::open(img) {
                Ok(p) => (Some(p), "used".to_string()),
                Err(why) => (None, format!("not used: {why}")),
            },
        };
        Ok(Store {
            cache,
            stage: stage.to_path_buf(),
            cache_full: std::sync::atomic::AtomicBool::new(false),
            prev,
            prev_dir: prev_img.and(prev_dir),
            prev_state,
            lookups: AtomicUsize::new(0),
            prev_hits: AtomicUsize::new(0),
        })
    }

    /// `(shaders in the previous library's table, lookups, lookups it served)`; `None` when no previous library was used
    pub fn previous_counts(&self) -> Option<(usize, usize, usize)> {
        self.prev.as_ref().map(|p| {
            (
                p.len(),
                self.lookups.load(Ordering::Relaxed),
                self.prev_hits.load(Ordering::Relaxed),
            )
        })
    }

    fn cache_path(&self, digest: &str) -> PathBuf {
        self.cache.join(&digest[..2]).join(digest)
    }

    pub fn stage_path(&self, digest: &str) -> PathBuf {
        self.stage.join(digest)
    }

    /// A hit: the entry's header is read (a damaged or short entry is a miss) and the entry is linked into the stage folder, from which the build reads it
    /// (speed-up B6) then, on a miss, the previous library: the entry is the old image's ranges of the shader's files
    pub fn lookup(&self, digest: &str) -> Option<Stored> {
        self.lookups.fetch_add(1, Ordering::Relaxed);
        let cp = self.cache_path(digest);
        let Some(mut st) = read_header(&cp) else {
            let hit = self.prev.as_ref()?.lookup(digest)?;
            self.prev_hits.fetch_add(1, Ordering::Relaxed);
            return Some(hit);
        };
        let sp = self.stage_path(digest);
        if sp.exists() || fs::hard_link(&cp, &sp).is_ok() {
            st.path = sp;
        }
        Some(st)
    }

    /// The stage entry of a shader the cache must not take (`commit` with `cacheable` false): a name that is not a digest, so the library's digests table leaves it out too
    /// (the next build makes it again, as it does when the cache never had it)
    pub fn uncached_stage_path(&self, digest: &str) -> PathBuf {
        self.stage.join(format!("{digest}.nc"))
    }

    /// A finished shader: its entry is written whole into the stage folder and, when `cacheable`, linked into the cache (an entry that is already there is left: the same digest is the same bytes)
    pub fn commit(
        &self,
        digest: &str,
        meta: &Meta,
        files: &[(String, Vec<u8>)],
        cacheable: bool,
    ) -> std::io::Result<Stored> {
        let bytes = encode(meta, files);
        let sp = if cacheable {
            self.stage_path(digest)
        } else {
            self.uncached_stage_path(digest)
        };
        write_atomic(&sp, &bytes)?;
        if cacheable {
            let cp = self.cache_path(digest);
            if let Some(dir) = cp.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if !cp.exists() {
                let mut tmp = cp.as_os_str().to_owned();
                tmp.push(tmp_suffix());
                let tmp = PathBuf::from(tmp);
                let linked = fs::hard_link(&sp, &tmp).is_ok() || fs::copy(&sp, &tmp).is_ok();
                if !linked || fs::rename(&tmp, &cp).is_err() {
                    let _ = fs::remove_file(&tmp);
                    // the cache could not take it (a full drive): the build goes on from its stage folder
                    self.cache_full
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        read_header(&sp)
            .ok_or_else(|| std::io::Error::other("a stage entry just written does not read back"))
    }

    pub fn remove_stage(&self) {
        let _ = fs::remove_dir_all(&self.stage);
    }

    /// Entries of other chains cannot hit (the chain is in the digest): their folders go. Returns the names removed. Stray temporary files (older than an hour) of killed builds go too.
    pub fn sweep_other_chains(cas: &Path, chain: &str) -> Vec<String> {
        let keep = format!("cas-{chain}.dmsb");
        let mut gone = Vec::new();
        if let Ok(rd) = fs::read_dir(cas) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("cas-")
                    && name.ends_with(".dmsb")
                    && name != keep
                    && fs::remove_dir_all(e.path()).is_ok()
                {
                    gone.push(name);
                }
            }
        }
        let cache = cas.join(&keep);
        if let Ok(rd) = fs::read_dir(&cache) {
            for d in rd.flatten() {
                if let Ok(files) = fs::read_dir(d.path()) {
                    for f in files.flatten() {
                        // a stray temporary file of a killed build; one younger than an hour may be another build's, being written right now
                        let old = f
                            .metadata()
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.elapsed().ok())
                            .map(|a| a.as_secs() >= 3600)
                            .unwrap_or(false);
                        if old && f.file_name().to_string_lossy().contains(".tmp-") {
                            let _ = fs::remove_file(f.path());
                        }
                    }
                }
            }
        }
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ibsb-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn commit_lookup_and_prune() {
        let d = tmp("a");
        let digest = "ab".to_string() + &"0".repeat(62);
        let files = vec![
            ("k_b.json".to_string(), b"{}".to_vec()),
            ("k_b.metal".to_string(), vec![7u8; 1000]),
        ];
        let meta = Meta {
            c1: vec!["k_b.metal".into()],
            cb: Some(("k".into(), "0:48".into())),
            tables: true,
            locs: vec![0, 3],
        };
        let s1 = Store::open(&d.join("cas"), "c1", &d.join("stage1")).unwrap();
        assert!(s1.lookup(&digest).is_none());
        let st = s1.commit(&digest, &meta, &files, true).unwrap();
        assert_eq!(st.meta, meta);
        assert_eq!(st.read("k_b.metal").unwrap().unwrap(), vec![7u8; 1000]);
        assert!(st.read("nope").is_none());
        // a second build sees the entry, links it into its own stage folder, and keeps the bytes after the cache entry is deleted (prune-safe)
        let s2 = Store::open(&d.join("cas"), "c1", &d.join("stage2")).unwrap();
        let hit = s2.lookup(&digest).expect("hit");
        assert_eq!(hit.files.len(), 2);
        assert!(hit.path.starts_with(d.join("stage2")));
        fs::remove_file(s2.cache_path(&digest)).unwrap();
        assert_eq!(hit.read_all().unwrap(), files);
        assert!(s2.lookup(&digest).is_none());
        // a truncated entry is a miss
        let tr = s2.cache_path(&digest);
        fs::create_dir_all(tr.parent().unwrap()).unwrap();
        let bytes = encode(&meta, &files);
        fs::write(&tr, &bytes[..bytes.len() - 5]).unwrap();
        assert!(s2.lookup(&digest).is_none());
        // other chains are swept
        fs::create_dir_all(d.join("cas").join("cas-old.dmsb")).unwrap();
        assert_eq!(
            Store::sweep_other_chains(&d.join("cas"), "c1"),
            vec!["cas-old.dmsb".to_string()]
        );
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_full_drive_is_told_apart() {
        assert!(super::is_disk_full(&std::io::Error::from_raw_os_error(28)));
        assert!(super::is_disk_full(&std::io::Error::from(
            std::io::ErrorKind::StorageFull
        )));
        assert!(!super::is_disk_full(&std::io::Error::from_raw_os_error(13)));
        assert!(!super::is_disk_full(&std::io::Error::other("no")));
    }
}
