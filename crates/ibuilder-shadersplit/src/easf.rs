//! EA's archives (the legacy FIFA engine, "RNA": FIFA 18 0100F7B002340000): BIG4 archives, EASF encrypted files, EA chunk containers and NXSHADER (`.fxo`) programs. Port of nxnative's
//! `dm/gfx/tools/easf/` (cb6eb28b, PR #1696; adapted there from ohxiaoguang/Touchline23, MIT, verified on FIFA 23) and its `ea_jobs` / `EA_ARCHIVE` branch of `scan_file_in`.
//!
//! * BIG4: `BIG4`, u32 (not read), u32 BE member count, u32 BE end of the index; then per member {u32 BE offset, u32 BE size, latin-1 name, NUL}. Only the index is read, then the first
//!   bytes of each member: only members that are EA containers ([`recognizes`]) are opened, the rest (audio banks, textures) are never read.
//! * EASF: `EASF`, u32 BE plaintext size, an 8-character key ID, SHA-256 of the first 32 plaintext bytes; AES-128-CBC ciphertext from +0x30 (IV = the key, PKCS#7 padding). The key is in
//!   the game's own `main`, as a record `{char id[8]; u8 key[16]}`: every occurrence of the ID is a candidate, and only one that decrypts the first two blocks to the header's digest is
//!   used (no address, build ID or key file). Keys are never logged or kept in a result: the log names a key by [`fingerprint`] (the first 16 hex digits of its SHA-256).
//! * chunk containers (`chunkzip`, `chunzstd`, `chunlzma`, `chunkref`): magic, u32 BE version 2, total size, chunk size, chunk count, alignment, 12 zero bytes; per block {u32 BE stored size,
//!   u32 BE type} placed so that the block's data starts on the alignment. Types 1 raw deflate, 6 zstd (2 RefPack, 5 LZMA are not decoded here); a block whose stored size is its output
//!   size is stored as is. A chunk container is decoded only when its first block's first bytes are an EA container (a BIG4 of `.fxo` files); one whose first block is LZMA or RefPack
//!   cannot be looked into and is counted (the split's log names how many), never decoded. In the RomFS itself only `chunkzip` containers are looked into, as upstream (`walk`).
//! * NXSHADER: `NXSHADER`, 8 version bytes, `GLSL` + u32 LE size of the one standard GLSLC output (magic 0x19866891) that follows at +0x18 (upstream reads it as a block from +0x10; FIFA 18's
//!   sizes end exactly at the output's end), then `REFL` (reflection, not read). The GLSLC
//!   output is read by [`glslc_shaders`]: its stages, compiled as one program, carry `#var` labels and pair as the program they are (see below).
//!
//! Pairs: upstream tags these `#NXSHADER@<offset>` and lists the tag in `SINGLE_STAGE`, so each of its vertex shaders is paired with a cover of all pixel shaders. Here the GLSLC reader's
//! labels are kept (`…#NXSHADER#GLSLC@0x18#var`): a `.fxo` is one program compiled by GLSLC, its vertex and pixel shader pair as that program (`var_pairs`), so `vs_pad` pads a vertex
//! shader to the pixel shader it is drawn with. The keys are the same either way; `shader_pairs.tsv` is not upstream's (see `docs/reference/shader-split.md`).

use crate::error::{Failure, Res, SplitError};
use crate::glslc::glslc_shaders;
use crate::red;
use crate::scan::FileRef;
use crate::scan_types::{Ctx, Extra, FileResult, Shader};
use crate::source::{MemSource, Source};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

pub const BIG4: &[u8; 4] = b"BIG4";
pub const EASF: &[u8; 4] = b"EASF";
pub const NXSHADER: &[u8; 8] = b"NXSHADER";
/// the chunk containers' magics and the block type they are named after
pub const CHUNK_MAGICS: [&[u8; 8]; 4] = [b"chunkzip", b"chunzstd", b"chunlzma", b"chunkref"];
/// EASF header: magic, size, key ID, digest
pub const EASF_HEADER: usize = 0x30;
/// the plaintext bytes the EASF digest covers
const EASF_PROBE: usize = 32;
/// chunk container header, up to the first block record
const CHUNK_HEADER: usize = 0x28;
/// archives in archives deeper than this are refused (upstream: eight levels)
const MAX_DEPTH: usize = 8;
/// members of one BIG4 archive per job
const RANGE: usize = 1024;
/// the largest BIG4 index read (FIFA 18's largest, data1.big's 23,394 members, is about 1.5 MB)
const MAX_INDEX: u64 = 64 << 20;

/// A chunk container's magic
pub fn is_chunk(head: &[u8]) -> bool {
    head.len() >= 8 && CHUNK_MAGICS.iter().any(|m| head[..8] == m[..])
}

/// `reader.recognizes`: the first bytes of an EA container (an archive, an encrypted file, a chunk container, a program)
pub fn recognizes(head: &[u8]) -> bool {
    head.starts_with(BIG4) || head.starts_with(EASF) || head.starts_with(NXSHADER) || is_chunk(head)
}

fn be32(d: &[u8], o: usize) -> Result<u32, String> {
    d.get(o..o + 4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
        .ok_or_else(|| format!("truncated at {o:#x}"))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// AES-128 CBC: `crate::aes`
use crate::aes;

// ---- EASF and its key ------------------------------------------------------------------------------------------------------------

/// `keyfind.fingerprint`: a stable, non-secret name of a key for logs (the first 16 hex digits of its SHA-256)
pub fn fingerprint(key: &[u8; 16]) -> String {
    hex(&Sha256::digest(key)[..8])
}

/// `keyfind.parse_header`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EasfHeader {
    pub size: usize,
    pub key_id: [u8; 8],
    pub digest: [u8; 32],
}

pub fn easf_header(d: &[u8]) -> Result<EasfHeader, String> {
    if !d.starts_with(EASF) {
        return Err("not an EASF container".into());
    }
    if d.len() < EASF_HEADER + EASF_PROBE || !(d.len() - EASF_HEADER).is_multiple_of(16) {
        return Err(format!("invalid EASF length ({} bytes)", d.len()));
    }
    let key_id: [u8; 8] = d[8..16].try_into().unwrap();
    if key_id.iter().any(|&c| !(0x20..0x7f).contains(&c)) {
        return Err("EASF key ID is not printable text".into());
    }
    Ok(EasfHeader {
        size: be32(d, 4)? as usize,
        key_id,
        digest: d[16..48].try_into().unwrap(),
    })
}

/// `keyfind.confirms`: whether `key` decrypts the container's first two blocks to the header's digest
pub fn confirms(d: &[u8], key: &[u8; 16]) -> bool {
    let Ok(h) = easf_header(d) else { return false };
    let plain = aes::Aes128::new(key).cbc_decrypt(key, &d[EASF_HEADER..EASF_HEADER + EASF_PROBE]);
    Sha256::digest(&plain)[..] == h.digest
}

/// The plaintext of an EASF container with a key that [`confirms`] it (the size is checked against the ciphertext and the digest again)
pub fn easf_decrypt(d: &[u8], key: &[u8; 16]) -> Result<Vec<u8>, String> {
    let h = easf_header(d)?;
    let mut plain = aes::Aes128::new(key).cbc_decrypt(key, &d[EASF_HEADER..]);
    if !(EASF_PROBE <= h.size && h.size <= plain.len())
        || Sha256::digest(&plain[..EASF_PROBE])[..] != h.digest
    {
        return Err("EASF plaintext prefix verification failed".into());
    }
    plain.truncate(h.size);
    Ok(plain)
}

/// `keyfind.records`: (offset, the 16 bytes after it) of every occurrence of the key ID in a module image
pub fn key_records(image: &[u8], key_id: &[u8; 8]) -> Vec<(usize, [u8; 16])> {
    memchr::memmem::find_iter(image, key_id)
        .filter_map(|at| {
            image
                .get(at + 8..at + 24)
                .map(|k| (at, k.try_into().unwrap()))
        })
        .collect()
}

/// Where a version's `main` is, or (the tests) its image
enum Module {
    Path(PathBuf),
    Image(Vec<u8>),
}

/// (version, key ID) → the keys found for it
type KeyCache = HashMap<(usize, [u8; 8]), Vec<[u8; 16]>>;

/// `keyfind.Keyring`, shared by the jobs: EASF keys found in the module of the version a file belongs to. A module is loaded only when an EASF container needs a key not found yet, and
/// dropped once searched; the verified keys of an ID are kept for the split (in memory only).
pub struct Keyring {
    modules: Vec<Module>,
    keys: Mutex<KeyCache>,
    notes: Mutex<BTreeSet<String>>,
}

impl Keyring {
    /// one `main` per version (the base game first); add-on content uses the last one
    pub fn new(mains: Vec<PathBuf>) -> Keyring {
        Keyring {
            modules: mains.into_iter().map(Module::Path).collect(),
            keys: Mutex::new(HashMap::new()),
            notes: Mutex::new(BTreeSet::new()),
        }
    }

    /// a keyring over module images in memory (one per version)
    pub fn with_images(images: Vec<Vec<u8>>) -> Keyring {
        Keyring {
            modules: images.into_iter().map(Module::Image).collect(),
            keys: Mutex::new(HashMap::new()),
            notes: Mutex::new(BTreeSet::new()),
        }
    }

    /// what the key search found, key-free (key IDs, where, candidates, fingerprints), sorted
    pub fn notes(&self) -> Vec<String> {
        self.notes.lock().unwrap().iter().cloned().collect()
    }

    /// A key that decrypts this EASF container (the key of its ID in the version's module). The error never holds key bytes
    pub fn resolve(&self, version: usize, d: &[u8]) -> Result<[u8; 16], String> {
        let h = easf_header(d)?;
        let v = version.min(self.modules.len().saturating_sub(1));
        let name = String::from_utf8_lossy(&h.key_id).into_owned();
        // the lock is held during a search, so concurrent jobs do not load the module several times
        let mut keys = self.keys.lock().unwrap();
        if let Some(k) = keys
            .get(&(v, h.key_id))
            .and_then(|ks| ks.iter().find(|k| confirms(d, k)))
        {
            return Ok(*k);
        }
        let (image, what) = match self.modules.get(v) {
            None => {
                return Err(format!(
                    "no module to find the key of EASF key ID {name:?} in"
                ));
            }
            Some(Module::Image(i)) => {
                (std::borrow::Cow::Borrowed(i.as_slice()), "main".to_string())
            }
            Some(Module::Path(p)) => {
                let m = ibuilder_nso::read(p)
                    .map_err(|e| format!("EASF key ID {name:?}: {}: {e}", p.display()))?;
                let what = format!(
                    "{} (build {})",
                    p.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    hex(&m.build_id[..16])
                );
                (std::borrow::Cow::Owned(m.image), what)
            }
        };
        let found = key_records(&image, &h.key_id);
        let mut notes = self.notes.lock().unwrap();
        for (at, key) in &found {
            if confirms(d, key) {
                keys.entry((v, h.key_id)).or_default().push(*key);
                notes.insert(format!(
                    "key ID {name:?} found in {what} at {at:#x}, {} candidate(s); key sha256[:16]={}",
                    found.len(),
                    fingerprint(key)
                ));
                return Ok(*key);
            }
        }
        notes.insert(format!(
            "{what}: {} record(s) named {name:?}, none verifies",
            found.len()
        ));
        Err(format!(
            "no record of EASF key ID {name:?} in {what} decrypts it ({} candidate(s))",
            found.len()
        ))
    }
}

// ---- BIG4 --------------------------------------------------------------------------------------------------------------------------

/// One member of a BIG4 archive
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Big4Entry {
    pub offset: u64,
    pub size: u64,
    pub name: String,
}

/// `reader.entries`: a BIG4 index, read without the archive's payload; every member is checked to lie in the archive after the index
pub fn big4_entries(src: &dyn Source) -> Result<Vec<Big4Entry>, String> {
    let length = src.len();
    let head = src.read(0, 16).map_err(|e| format!("OSError: {e}"))?;
    if head.len() != 16 || !head.starts_with(BIG4) {
        return Err("invalid BIG4 header".into());
    }
    let (count, end) = (be32(&head, 8)? as u64, be32(&head, 12)? as u64);
    if !(16 <= end && end <= length) || count > (end - 16) / 9 {
        return Err("BIG4 index exceeds archive".into());
    }
    if end > MAX_INDEX {
        return Err(format!(
            "BIG4 index of {end} bytes is larger than {MAX_INDEX}"
        ));
    }
    let table = src
        .read(16, (end - 16) as usize)
        .map_err(|e| format!("OSError: {e}"))?;
    if table.len() as u64 != end - 16 {
        return Err("truncated BIG4 index".into());
    }
    let mut at = 0usize;
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        if at + 9 > table.len() {
            return Err("truncated BIG4 entry".into());
        }
        let (offset, size) = (be32(&table, at)? as u64, be32(&table, at + 4)? as u64);
        let z = memchr::memchr(0, &table[at + 8..]).ok_or("unterminated BIG4 name")? + at + 8;
        if offset < end || offset > length || size > length - offset {
            return Err("BIG4 member exceeds archive".into());
        }
        out.push(Big4Entry {
            offset,
            size,
            name: table[at + 8..z].iter().map(|&b| b as char).collect(),
        });
        at = z + 1;
    }
    Ok(out)
}

/// A byte range of another source
struct Slice<'a> {
    src: &'a dyn Source,
    off: u64,
    len: u64,
}

impl Source for Slice<'_> {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        if off >= self.len {
            return Ok(0);
        }
        let n = buf.len().min((self.len - off) as usize);
        self.src.read_at(self.off + off, &mut buf[..n])
    }
}

// ---- chunk containers -------------------------------------------------------------------------------------------------------------

/// A chunk container's header
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkHeader {
    pub magic: [u8; 8],
    pub total: usize,
    pub chunk: usize,
    pub count: usize,
    pub align: usize,
}

pub fn chunk_header(d: &[u8]) -> Result<ChunkHeader, String> {
    if d.len() < CHUNK_HEADER + 8 || !is_chunk(d) {
        return Err("truncated chunk container header".into());
    }
    let magic: [u8; 8] = d[..8].try_into().unwrap();
    let (ver, total, chunk, count, align) = (
        be32(d, 8)?,
        be32(d, 12)? as usize,
        be32(d, 16)? as usize,
        be32(d, 20)? as usize,
        be32(d, 24)? as usize,
    );
    let m = String::from_utf8_lossy(&magic);
    if ver != 2
        || chunk == 0
        || !align.is_power_of_two()
        || align < 8
        || count != total.div_ceil(chunk)
    {
        return Err(format!(
            "invalid {m} geometry (version {ver}, {total} bytes in {count} chunk(s) of {chunk}, alignment {align})"
        ));
    }
    if total > crate::inflate::MAX_DECODED {
        return Err(format!(
            "{m}: {}",
            crate::inflate::cap_error(crate::inflate::MAX_DECODED)
        ));
    }
    Ok(ChunkHeader {
        magic,
        total,
        chunk,
        count,
        align,
    })
}

/// where block record `o` is: the record ends where the block's data starts, on the alignment
fn record_at(o: usize, align: usize) -> usize {
    (o + 8).div_ceil(align) * align - 8
}

fn block_kind(typ: u32) -> &'static str {
    match typ {
        1 => "deflate",
        2 => "RefPack",
        5 => "LZMA",
        6 => "zstd",
        _ => "unknown",
    }
}

/// One block's output: stored (its stored size is its output size), raw deflate or zstd; `want` bytes, or the first `max` of them
fn block(
    data: &[u8],
    typ: u32,
    want: usize,
    max: Option<usize>,
    zstd: &dyn crate::zstd::Zstd,
) -> Result<Vec<u8>, String> {
    let n = max.map_or(want, |m| m.min(want));
    if data.len() == want {
        return Ok(data[..n].to_vec());
    }
    let out = match typ {
        1 => {
            let r = crate::inflate::run(data, crate::inflate::Wrap::Raw, Some(n))?;
            if max.is_none() && !r.eof {
                return Err("deflate block does not end".into());
            }
            r.out
        }
        6 => zstd.decompress(data, None, Some(n))?,
        2 | 5 => {
            return Err(format!(
                "block type {typ} ({}) is not decoded by this port",
                block_kind(typ)
            ));
        }
        t => return Err(format!("unknown block type {t}")),
    };
    if out.len() != n {
        return Err(format!(
            "{} block makes {} of {n} bytes",
            block_kind(typ),
            out.len()
        ));
    }
    Ok(out)
}

/// What the first bytes of a chunk container's output are
#[derive(Debug, PartialEq, Eq)]
pub enum Peek {
    /// an EA container: worth decoding
    Archive,
    /// anything else (a texture, a model): not decoded
    Other,
    /// its first block is LZMA or RefPack (not decoded here), or its header is not one this reader knows: not looked into
    Unread(String),
}

/// The first output bytes of a chunk container, from its header and first block only
pub fn chunk_peek(src: &dyn Source, zstd: &dyn crate::zstd::Zstd) -> Result<Peek, String> {
    let head = src
        .read(0, CHUNK_HEADER + 8)
        .map_err(|e| format!("OSError: {e}"))?;
    let m = String::from_utf8_lossy(&head[..8.min(head.len())]).into_owned();
    let h = match chunk_header(&head) {
        Ok(h) => h,
        Err(e) => {
            return Ok(Peek::Unread(format!(
                "{m} container(s) not looked into: {e}"
            )));
        }
    };
    if h.count == 0 {
        return Ok(Peek::Other);
    }
    let o = record_at(CHUNK_HEADER, h.align);
    let rec = src.read(o as u64, 8).map_err(|e| format!("OSError: {e}"))?;
    let (sz, typ) = (be32(&rec, 0)? as usize, be32(&rec, 4)?);
    let want = h.chunk.min(h.total);
    if (o + 8 + sz) as u64 > src.len() {
        return Ok(Peek::Unread(format!(
            "{m} container(s) not looked into: the first block is outside the container ({sz} bytes)"
        )));
    }
    if sz != want && matches!(typ, 2 | 5) {
        return Ok(Peek::Unread(format!(
            "{m} container(s) not looked into: the first block is {} (type {typ}), which this port does not decode",
            block_kind(typ)
        )));
    }
    let data = src
        .read((o + 8) as u64, sz)
        .map_err(|e| format!("OSError: {e}"))?;
    match block(&data, typ, want, Some(8), zstd) {
        Ok(first) if recognizes(&first) => Ok(Peek::Archive),
        Ok(_) => Ok(Peek::Other),
        Err(e) => Ok(Peek::Unread(format!(
            "{m} container(s) not looked into: {e}"
        ))),
    }
}

/// A whole chunk container's output (`chunkzip.decode`, with the block records the FIFA 18 containers carry)
pub fn chunk_decode(d: &[u8], zstd: &dyn crate::zstd::Zstd) -> Result<Vec<u8>, String> {
    let h = chunk_header(d)?;
    let m = String::from_utf8_lossy(&h.magic).into_owned();
    let mut out = Vec::with_capacity(h.total);
    let mut o = CHUNK_HEADER;
    for i in 0..h.count {
        o = record_at(o, h.align);
        let (sz, typ) = (
            be32(d, o).map_err(|_| format!("{m}: block {i} record is outside the container"))?
                as usize,
            be32(d, o + 4)
                .map_err(|_| format!("{m}: block {i} record is outside the container"))?,
        );
        o += 8;
        if sz > d.len() - o {
            return Err(format!(
                "{m}: block {i} ({sz} bytes at {o:#x}) is outside the container"
            ));
        }
        let want = h.chunk.min(h.total - out.len());
        let b = block(&d[o..o + sz], typ, want, None, zstd)
            .map_err(|e| format!("{m}: block {i}: {e}"))?;
        out.extend_from_slice(&b);
        o += sz;
    }
    if out.len() != h.total {
        return Err(format!("{m}: size mismatch: {} != {}", out.len(), h.total));
    }
    Ok(out)
}

// ---- NXSHADER ------------------------------------------------------------------------------------------------------------------------

/// `fxo.glsl_block` + `fxo.programs`: the shaders of the GLSLC output in a `.fxo`'s GLSL block, labelled `<label>#NXSHADER#GLSLC@<offset>[#var]`
pub fn fxo_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    if d.len() < 0x18 || !d.starts_with(NXSHADER) {
        red!("{label}: truncated NXSHADER header");
    }
    if &d[0x10..0x14] != b"GLSL" {
        red!("{label}: NXSHADER without a GLSL block");
    }
    let size = u32::from_le_bytes(d[0x14..0x18].try_into().unwrap()) as usize;
    let end = 0x18usize.saturating_add(size);
    if size < 8 || end > d.len() {
        red!("{label}: NXSHADER GLSL block of {size} bytes exceeds the file");
    }
    glslc_shaders(&d[..end], &format!("{label}#NXSHADER"), false, &[])
}

// ---- the jobs ---------------------------------------------------------------------------------------------------------------------

/// One job: a top-level EA file whole, or a range of a top-level BIG4 archive's members
#[derive(Clone)]
pub struct EaJob {
    pub file: FileRef,
    /// the version whose `main` holds its keys (the base game 0; add-on content the last version)
    pub version: usize,
    /// None: the whole file
    pub entries: Option<Vec<Big4Entry>>,
}

impl EaJob {
    pub fn identity(&self) -> Vec<String> {
        let mut v = vec![format!(
            "ea:{}:{}:{}:v{}",
            self.file.root,
            self.file.rel,
            self.file.loc.len(),
            self.version
        )];
        if let Some(es) = &self.entries {
            v.extend(
                es.iter()
                    .map(|e| format!("{}:{}:{}", e.offset, e.size, e.name)),
            );
        }
        v
    }
}

/// The EA plan: (the jobs, the files they read as (root, rel))
pub type EaPlan = (Vec<EaJob>, HashSet<(usize, String)>);

/// `ea_jobs(files, romfs, mains)`: every file whose first bytes are an EA container, a BIG4 archive in ranges of its members (an index that cannot be read leaves the archive as one job,
/// which reports it). `versions`: how many roots are game versions (the rest are add-on content)
pub fn plan(files: &[FileRef], versions: usize) -> Result<EaPlan, SplitError> {
    let mut jobs = Vec::new();
    let mut taken = HashSet::new();
    for f in files {
        let Ok(src) = f.loc.open() else { continue }; // scan_file reports unreadable loose files
        let Ok(head) = src.read(0, 8) else { continue };
        if !recognizes(&head) {
            continue;
        }
        taken.insert((f.root, f.rel.clone()));
        let version = if f.root < versions {
            f.root
        } else {
            versions.saturating_sub(1)
        };
        let whole = EaJob {
            file: f.clone(),
            version,
            entries: None,
        };
        if !head.starts_with(BIG4) {
            jobs.push(whole);
            continue;
        }
        match big4_entries(&src) {
            Ok(es) if !es.is_empty() => {
                for c in es.chunks(RANGE) {
                    jobs.push(EaJob {
                        entries: Some(c.to_vec()),
                        ..whole.clone()
                    });
                }
            }
            Ok(_) => {}
            Err(_) => jobs.push(whole),
        }
    }
    Ok((jobs, taken))
}

/// What the jobs share: the keyring, and the chunk containers that could not be looked into (kind → (count, the first one's label))
pub struct EaShared {
    pub ring: Keyring,
    unread: Mutex<BTreeMap<String, (usize, String)>>,
}

impl EaShared {
    pub fn new(ring: Keyring) -> EaShared {
        EaShared {
            ring,
            unread: Mutex::new(BTreeMap::new()),
        }
    }

    /// the log lines of the jobs that ran: the keys found (key-free), the containers not looked into
    pub fn notes(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .ring
            .notes()
            .into_iter()
            .map(|n| format!("EA archive: {n}"))
            .collect();
        for (kind, (n, first)) in self.unread.lock().unwrap().iter() {
            v.push(format!("EA archive: {n} {kind} (first: {first})"));
        }
        v
    }

    /// one more container of this kind (a text without per-file numbers) not looked into; the first label by name is kept, so the log is the same whatever order the jobs run in
    fn unread(&self, kind: &str, label: &str) {
        let mut u = self.unread.lock().unwrap();
        let e = u.entry(kind.to_string()).or_insert((0, label.to_string()));
        e.0 += 1;
        if label < e.1.as_str() {
            e.1 = label.to_string();
        }
    }
}

/// `reader.archive`: the shaders of one EA container (`src`), and of the EA containers in it. `outer`: `src` is bytes of the RomFS (a file, a member of its BIG4 archives), not
/// decoded data: there, as upstream, of the chunk containers only `chunkzip` ones are looked into (FIFA 18's 62,811 `chunzstd` members are textures and models; peeking at their
/// first blocks costs 30 s of zstd on an M1 Pro for nothing); in decoded data (an EASF plaintext) every kind is looked into
#[allow(clippy::too_many_arguments)]
fn walk(
    cx: &Ctx<'_>,
    sh: &EaShared,
    version: usize,
    src: &dyn Source,
    label: &str,
    depth: usize,
    outer: bool,
    found: &mut Vec<Shader>,
) -> Res<()> {
    if depth > MAX_DEPTH {
        red!("{label}: EA archive nesting exceeds eight levels");
    }
    let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
    let head = src.read(0, 8).map_err(io)?;
    if head.starts_with(EASF) {
        let d = src.read_all().map_err(io)?;
        let key = sh
            .ring
            .resolve(version, &d)
            .map_err(|e| Failure::Red(format!("{label}: {e}")))?;
        let plain = easf_decrypt(&d, &key).map_err(|e| Failure::Red(format!("{label}: {e}")))?;
        drop(d);
        return walk(
            cx,
            sh,
            version,
            &MemSource(&plain),
            label,
            depth + 1,
            false,
            found,
        );
    }
    if is_chunk(&head) {
        if outer && head[..8] != CHUNK_MAGICS[0][..] {
            sh.unread(
                &format!(
                    "{} container(s) in the RomFS not opened (as upstream, only chunkzip ones are)",
                    String::from_utf8_lossy(&head[..8])
                ),
                label,
            );
            return Ok(());
        }
        match chunk_peek(src, cx.zstd).map_err(|e| Failure::Red(format!("{label}: {e}")))? {
            Peek::Other => return Ok(()),
            Peek::Unread(why) => {
                // the kind is the magic and what stopped the peek, without the numbers of this one container
                let kind = if why.contains("(type ") {
                    why.as_str()
                } else {
                    why.split(" (").next().unwrap_or(&why)
                };
                sh.unread(kind, label);
                return Ok(());
            }
            Peek::Archive => {}
        }
        let d = src.read_all().map_err(io)?;
        let plain = chunk_decode(&d, cx.zstd).map_err(|e| Failure::Red(format!("{label}: {e}")))?;
        drop(d);
        return walk(
            cx,
            sh,
            version,
            &MemSource(&plain),
            label,
            depth + 1,
            false,
            found,
        );
    }
    if head.starts_with(NXSHADER) {
        let d = src.read_all().map_err(io)?;
        found.extend(fxo_shaders(&d, label)?);
        return Ok(());
    }
    if !head.starts_with(BIG4) {
        return Ok(()); // a valid decoded resource that is no EA container
    }
    let entries = big4_entries(src).map_err(|e| Failure::Red(format!("{label}: {e}")))?;
    members(cx, sh, version, src, &entries, label, depth, outer, found)
}

/// the members of a BIG4 archive that are EA containers
#[allow(clippy::too_many_arguments)]
fn members(
    cx: &Ctx<'_>,
    sh: &EaShared,
    version: usize,
    src: &dyn Source,
    entries: &[Big4Entry],
    label: &str,
    depth: usize,
    outer: bool,
    found: &mut Vec<Shader>,
) -> Res<()> {
    for e in entries {
        if e.size < 8 {
            continue;
        }
        let m = Slice {
            src,
            off: e.offset,
            len: e.size,
        };
        let head = m
            .read(0, 8)
            .map_err(|x| Failure::Red(format!("OSError: {x}")))?;
        if recognizes(&head) {
            walk(
                cx,
                sh,
                version,
                &m,
                &format!("{label}#{}", e.name),
                depth + 1,
                outer,
                found,
            )?;
        }
    }
    Ok(())
}

fn run(cx: &Ctx<'_>, job: &EaJob, sh: &EaShared) -> Res<FileResult> {
    let src = job
        .file
        .loc
        .open()
        .map_err(|x| Failure::Red(format!("OSError: {x}")))?;
    let label = format!("/{}", job.file.rel);
    let mut found = Vec::new();
    match &job.entries {
        None => walk(cx, sh, job.version, &src, &label, 0, true, &mut found)?,
        Some(es) => members(cx, sh, job.version, &src, es, &label, 0, true, &mut found)?,
    }
    crate::scan::finish(cx, &job.file.rel, found, [], crate::unity::Trees::new())
}

/// The job as `scan_file` runs it: an EA container that cannot be read leaves the job's shaders out (`[RED] <why> (its shaders are left out)`)
pub fn scan(cx: &Ctx<'_>, job: &EaJob, sh: &EaShared) -> Result<FileResult, String> {
    match run(cx, job, sh) {
        Ok(r) => Ok(r),
        Err(Failure::Unsupported(m)) => Err(m),
        Err(e) => {
            let rel = job.file.rel.clone();
            let msg = e.to_string();
            let why = if msg
                .trim_start_matches('/')
                .starts_with(rel.trim_start_matches('/'))
            {
                msg
            } else {
                format!("{rel}: {msg}")
            };
            cx.print(&format!("[RED] {why} (its shaders are left out)"));
            Ok(FileResult {
                rel,
                uniq: vec![],
                pairs: vec![],
                extra: Extra::Unreadable { why, count: 1 },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_placement() {
        assert_eq!(record_at(0x28, 16), 0x28);
        assert_eq!(record_at(0x31, 16), 0x38);
        assert_eq!(record_at(0x38, 16), 0x38);
    }
}
