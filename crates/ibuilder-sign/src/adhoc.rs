//! The ad-hoc signer: `adhoc_sign.py` `sign()` / `verify()`, planned first (nothing is written until the whole
//! signature is computed) and then applied to a `Vec<u8>` or, in place, to a file.
use crate::error::{Error, fmt_err};
use crate::macho::{
    self, LC_CODE_SIGNATURE, OFF_NCMDS, OFF_SIZEOFCMDS, SEG_LINKEDIT, rd_u32, wr_u32, wr_u64,
};
use crate::source::{FileSource, Source};
use crate::{
    CD_FIXED, CD_VERSION, CS_ADHOC, CSMAGIC_CODEDIRECTORY, CSMAGIC_EMBEDDED_SIGNATURE,
    HASH_TYPE_SHA256, PAGE, PAGE_LOG2,
};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

/// Pages hashed per task (1 MiB at 4 KiB pages): one read and one buffer per task.
const CHUNK_PAGES: usize = 256;
/// The header buffer: the first MiB of the image (headers are ~33 KB with `-headerpad,0x8000`).
const HDR_MIN: u64 = 1 << 20;
/// Load commands larger than this are refused instead of read into memory.
const HDR_MAX: u64 = 64 << 20;

/// Knobs that default to the Python's behaviour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SignOptions {
    /// When a signature already in the file is dropped, also shrink `__LINKEDIT`'s `filesize` to the new end of the
    /// file. adhoc_sign.py only ever grows it (`max(old, new)`), so re-signing a file whose previous signature was
    /// *larger* (e.g. a zsign/CMS one) leaves `__LINKEDIT` reaching past the end of the file, which `codesign`
    /// rejects ("main executable failed strict validation"). Off by default: byte-identical to the Python.
    pub shrink_linkedit: bool,
}

/// What `sign()` returns in the Python (`{"identifier", "code_limit", "n_code", "datasize", "filelen"}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignInfo {
    pub identifier: String,
    pub code_limit: u64,
    pub n_code: usize,
    pub datasize: u32,
    pub filelen: u64,
}

/// What `verify()` returns in the Python.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyInfo {
    pub flag_adhoc: bool,
    pub n_code: u32,
    pub pages_ok: u32,
    pub code_limit: u32,
    pub dataoff: u32,
    pub matches: bool,
}

fn align(x: u64, a: u64) -> u64 {
    (x + a - 1) & !(a - 1)
}

/// A fully computed signature: the patched header prefix, where the retained data ends, and the blob.
#[derive(Debug)]
pub struct Plan {
    /// The patched start of the image (`min(header buffer, base_len)` bytes).
    header: Vec<u8>,
    /// Length of the image that stays (the whole file, or the old signature's offset when one is dropped).
    base_len: u64,
    code_limit: u64,
    blob: Vec<u8>,
    info: SignInfo,
}

impl Plan {
    pub fn info(&self) -> &SignInfo {
        &self.info
    }
    /// The SuperBlob that will be appended at `code_limit`.
    pub fn blob(&self) -> &[u8] {
        &self.blob
    }

    /// Apply to an in-memory image (the same bytes the Python builds in its `bytearray`).
    pub fn apply_to_vec(&self, data: &mut Vec<u8>) {
        data.truncate(self.base_len as usize);
        data.resize(self.code_limit as usize, 0);
        data[..self.header.len()].copy_from_slice(&self.header);
        data.extend_from_slice(&self.blob);
    }

    /// Apply in place. Order: drop the old tail, zero-extend to `codeLimit`, write the blob, and the header last,
    /// so an interrupted run never leaves a header pointing at a blob that is not there.
    pub fn apply_to_file(&self, file: &std::fs::File) -> io::Result<()> {
        file.set_len(self.base_len)?;
        file.set_len(self.code_limit)?;
        file.write_all_at(&self.blob, self.code_limit)?;
        file.write_all_at(&self.header, 0)?;
        Ok(())
    }
}

fn read_header<S: Source + ?Sized>(src: &S) -> Result<Vec<u8>, Error> {
    let len = src.len();
    let mut n = len.min(HDR_MIN);
    let mut h = vec![0u8; n as usize];
    src.read_at(0, &mut h)?;
    if h.len() >= OFF_SIZEOFCMDS + 4 {
        let need = 32 + rd_u32(&h, OFF_SIZEOFCMDS)? as u64 + 16;
        if need > HDR_MAX {
            return fmt_err(format!("load commands of {need:#x} bytes: refused"));
        }
        if need > n && n < len {
            n = len.min(align(need, PAGE as u64));
            h = vec![0u8; n as usize];
            src.read_at(0, &mut h)?;
        }
    }
    Ok(h)
}

/// Bytes `[lo, lo + buf.len())` of the image as it will be signed: the patched header, then the file, then zero
/// padding past `base_len`.
fn fill_view<S: Source + ?Sized>(
    src: &S,
    header: &[u8],
    base_len: u64,
    lo: u64,
    buf: &mut [u8],
) -> Result<(), Error> {
    buf.fill(0);
    let hi = lo + buf.len() as u64;
    let hdr = header.len() as u64;
    if lo < hdr {
        let e = hi.min(hdr);
        buf[..(e - lo) as usize].copy_from_slice(&header[lo as usize..e as usize]);
    }
    let a = lo.max(hdr);
    let b = hi.min(base_len);
    if a < b {
        src.read_at(a, &mut buf[(a - lo) as usize..(b - lo) as usize])?;
    }
    Ok(())
}

/// SHA-256 of every `PAGE`-sized page of `[0, code_limit)` (the last one unpadded), `n_code * 32` bytes.
fn hash_pages<S: Source + ?Sized>(
    src: &S,
    header: &[u8],
    base_len: u64,
    code_limit: u64,
    n_code: usize,
) -> Result<Vec<u8>, Error> {
    let n_chunks = n_code.div_ceil(CHUNK_PAGES);
    let parts: Vec<Result<Vec<u8>, Error>> = (0..n_chunks)
        .into_par_iter()
        .map_init(
            || vec![0u8; CHUNK_PAGES * PAGE],
            |buf, c| {
                let p0 = c * CHUNK_PAGES;
                let p1 = (p0 + CHUNK_PAGES).min(n_code);
                let lo = (p0 * PAGE) as u64;
                let hi = ((p1 * PAGE) as u64).min(code_limit);
                let chunk = &mut buf[..(hi - lo) as usize];
                fill_view(src, header, base_len, lo, chunk)?;
                let mut out = Vec::with_capacity((p1 - p0) * 32);
                for page in chunk.chunks(PAGE) {
                    out.extend_from_slice(&Sha256::digest(page));
                }
                Ok(out)
            },
        )
        .collect();
    let mut all = Vec::with_capacity(n_code * 32);
    for p in parts {
        all.extend_from_slice(&p?);
    }
    Ok(all)
}

/// Compute the signature of `src` without touching it (`sign()` steps 1-5). `identifier` empty or `None` falls
/// back to the dylib's `LC_ID_DYLIB` name, then to `default_name` (the Python's `os.path.basename(path)`).
pub fn plan<S: Source + ?Sized>(
    src: &S,
    identifier: Option<&str>,
    default_name: &str,
) -> Result<Plan, Error> {
    plan_with(src, identifier, default_name, &SignOptions::default())
}

/// [`plan`] with options.
pub fn plan_with<S: Source + ?Sized>(
    src: &S,
    identifier: Option<&str>,
    default_name: &str,
    opts: &SignOptions,
) -> Result<Plan, Error> {
    let len = src.len();
    let mut header = read_header(src)?;
    let mut base_len = len;
    let mut info = macho::inspect(&header)?;
    if !info.segments.contains_key(SEG_LINKEDIT) {
        return fmt_err("no __LINKEDIT segment: nothing to grow for the signature");
    }

    // Drop a signature the file already carries (idempotent re-run); its LC_CODE_SIGNATURE slot is reused below.
    if let Some(sig_lo) = info.sig_lo {
        let dataoff = rd_u32(&header, sig_lo + 8)? as u64;
        let datasize = rd_u32(&header, sig_lo + 12)? as u64;
        if dataoff != 0 && dataoff + datasize <= len {
            base_len = dataoff;
            header.truncate(header.len().min(base_len as usize));
            info = macho::inspect(&header)?;
        }
    }

    let name: String = match identifier {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => match &info.identifier {
            Some(s) if !s.is_empty() => s.clone(),
            _ => default_name.to_string(),
        },
    };
    let mut name_b = name.as_bytes().to_vec();
    name_b.push(0);

    let ncmds = rd_u32(&header, OFF_NCMDS)?;
    let sizeofcmds = rd_u32(&header, OFF_SIZEOFCMDS)?;

    // 1. An LC_CODE_SIGNATURE slot, written over the header slack (never inserted: that would shift sections).
    if info.sig_lo.is_none() {
        if macho::slack(&header)? < 16 {
            return fmt_err("no room for an LC_CODE_SIGNATURE command (headerpad too small)");
        }
        let lc_lo = 32 + sizeofcmds as usize;
        wr_u32(&mut header, lc_lo, LC_CODE_SIGNATURE)?;
        wr_u32(&mut header, lc_lo + 4, 16)?;
        wr_u32(&mut header, lc_lo + 8, 0)?;
        wr_u32(&mut header, lc_lo + 12, 0)?;
        wr_u32(&mut header, OFF_NCMDS, ncmds + 1)?;
        wr_u32(&mut header, OFF_SIZEOFCMDS, sizeofcmds + 16)?;
        info = macho::inspect(&header)?;
    }
    let lc_lo = info.sig_lo.expect("the slot was just written");

    // 2. Geometry. codeLimit is the 16-aligned end of the retained image; the SuperBlob follows it.
    let code_limit = align(base_len, 16);
    let n_code = code_limit.div_ceil(PAGE as u64) as usize;
    let cd_len = CD_FIXED + name_b.len() + n_code * 32;
    let super_inner = 20 + cd_len; // the SuperBlob's own length field (ld writes it unpadded)
    let super_len = align(super_inner as u64, 8); // the LC datasize is 8-aligned
    let to_u32 = |v: u64, what: &str| -> Result<u32, Error> {
        u32::try_from(v).map_err(|_| {
            Error::Format(format!(
                "{what} {v:#x} does not fit the 32-bit field of the format"
            ))
        })
    };
    let code_limit32 = to_u32(code_limit, "codeLimit")?;
    let super_len32 = to_u32(super_len, "signature size")?;

    // 3. Grow __LINKEDIT (before hashing: the hash covers the header, load commands included).
    let le = info.segments[SEG_LINKEDIT];
    let need = (code_limit + super_len).saturating_sub(le.fileoff);
    let old_filesize = if opts.shrink_linkedit && base_len < len {
        le.filesize.min(base_len.saturating_sub(le.fileoff)) // the dropped signature was inside __LINKEDIT's old extent
    } else {
        le.filesize
    };
    let new_filesize = old_filesize.max(need);
    // `new_filesize` is at least `__LINKEDIT`'s own filesize, which is the file's word: `x + 4095` wraps for a value near 2^64 (a panic in a debug build, a silently wrapped vmsize in a release one)
    let aligned = new_filesize
        .checked_add(PAGE as u64 - 1)
        .map(|v| v & !(PAGE as u64 - 1))
        .ok_or_else(|| {
            Error::Format(format!(
                "__LINKEDIT filesize {new_filesize:#x} overflows when aligned to a page"
            ))
        })?;
    let new_vmsize = le.vmsize.max(aligned);
    wr_u64(&mut header, le.lo + 48, new_filesize)?;
    wr_u64(&mut header, le.lo + 32, new_vmsize)?;

    // 4. Point the load command at the blob.
    wr_u32(&mut header, lc_lo + 8, code_limit32)?;
    wr_u32(&mut header, lc_lo + 12, super_len32)?;

    // 5. CodeDirectory v0x20400: fixed header, identifier, one SHA-256 per 4096-byte page.
    let mut cd = vec![0u8; cd_len];
    let be32 =
        |cd: &mut [u8], off: usize, v: u32| cd[off..off + 4].copy_from_slice(&v.to_be_bytes());
    be32(&mut cd, 0, CSMAGIC_CODEDIRECTORY);
    be32(&mut cd, 4, cd_len as u32);
    be32(&mut cd, 8, CD_VERSION);
    be32(&mut cd, 12, CS_ADHOC);
    be32(&mut cd, 16, (CD_FIXED + name_b.len()) as u32); // hashOffset
    be32(&mut cd, 20, CD_FIXED as u32); // identOffset
    be32(&mut cd, 24, 0); // nSpecialSlots
    be32(&mut cd, 28, n_code as u32); // nCodeSlots
    be32(&mut cd, 32, code_limit32); // codeLimit
    cd[36] = 32; // hashSize
    cd[37] = HASH_TYPE_SHA256;
    cd[38] = 0; // platform
    cd[39] = PAGE_LOG2;
    // spare2(40) scatterOffset(44) teamOffset(48) spare3(52) codeLimit64(56) execSegBase(64) execSegLimit(72)
    // execSegFlags(80): all zero, as `ld` writes them for a plain ad-hoc dylib.
    cd[CD_FIXED..CD_FIXED + name_b.len()].copy_from_slice(&name_b);
    let hash_off = CD_FIXED + name_b.len();
    let hashes = hash_pages(src, &header, base_len, code_limit, n_code)?;
    cd[hash_off..hash_off + hashes.len()].copy_from_slice(&hashes);

    // 6. Wrap it in the SuperBlob (count 1, one CodeDirectory at offset 20).
    let mut blob = vec![0u8; super_len as usize];
    blob[0..4].copy_from_slice(&CSMAGIC_EMBEDDED_SIGNATURE.to_be_bytes());
    blob[4..8].copy_from_slice(&(super_inner as u32).to_be_bytes());
    blob[8..12].copy_from_slice(&1u32.to_be_bytes());
    blob[12..16].copy_from_slice(&0u32.to_be_bytes()); // blob type: CodeDirectory slot 0
    blob[16..20].copy_from_slice(&20u32.to_be_bytes());
    blob[20..20 + cd_len].copy_from_slice(&cd);

    let filelen = code_limit + super_len;
    Ok(Plan {
        header,
        base_len,
        code_limit,
        blob,
        info: SignInfo {
            identifier: name,
            code_limit,
            n_code,
            datasize: super_len32,
            filelen,
        },
    })
}

/// Sign an in-memory image (the Python's `sign()` minus the file I/O).
pub fn sign_bytes(
    data: &mut Vec<u8>,
    identifier: Option<&str>,
    default_name: &str,
) -> Result<SignInfo, Error> {
    sign_bytes_with(data, identifier, default_name, &SignOptions::default())
}

/// [`sign_bytes`] with options.
pub fn sign_bytes_with(
    data: &mut Vec<u8>,
    identifier: Option<&str>,
    default_name: &str,
    opts: &SignOptions,
) -> Result<SignInfo, Error> {
    let p = plan_with(&*data, identifier, default_name, opts)?;
    p.apply_to_vec(data);
    Ok(p.info.clone())
}

/// Sign a file in place, streaming: the file is never held in memory.
pub fn sign_file(path: &Path, identifier: Option<&str>) -> Result<SignInfo, Error> {
    sign_file_with(path, identifier, &SignOptions::default())
}

/// [`sign_file`] with options.
pub fn sign_file_with(
    path: &Path,
    identifier: Option<&str>,
    opts: &SignOptions,
) -> Result<SignInfo, Error> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    let len = file.metadata()?.len();
    let src = FileSource { file, len };
    let default_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let p = plan_with(&src, identifier, &default_name, opts)?;
    p.apply_to_file(&src.file)?;
    Ok(p.info.clone())
}

/// Parsed fixed fields of a CodeDirectory (what `verify()` unpacks).
#[derive(Clone, Debug)]
pub struct CodeDirectoryView {
    pub flags: u32,
    pub hash_offset: u32,
    pub ident_offset: u32,
    pub n_special: u32,
    pub n_code: u32,
    pub code_limit: u32,
    pub hash_size: u8,
    pub hash_type: u8,
    pub page_log2: u8,
}

/// `verify()`: parse the signature back and re-hash every page, the way the kernel would.
pub fn verify_source<S: Source + ?Sized>(src: &S) -> Result<VerifyInfo, Error> {
    let len = src.len();
    let header = read_header(src)?;
    let info = macho::inspect(&header)?;
    let Some(sig_lo) = info.sig_lo else {
        return fmt_err("no LC_CODE_SIGNATURE");
    };
    let dataoff = rd_u32(&header, sig_lo + 8)?;
    let datasize = rd_u32(&header, sig_lo + 12)?;
    let b0 = (dataoff as u64).min(len);
    let b1 = (dataoff as u64 + datasize as u64).min(len);
    let mut blob = vec![0u8; (b1 - b0) as usize];
    src.read_at(b0, &mut blob)?;
    if blob.len() < 12 {
        return fmt_err("bad SuperBlob (shorter than its header)");
    }
    let be = |d: &[u8], o: usize| u32::from_be_bytes(d[o..o + 4].try_into().unwrap());
    let (smagic, _slen, count) = (be(&blob, 0), be(&blob, 4), be(&blob, 8));
    if smagic != CSMAGIC_EMBEDDED_SIGNATURE || count != 1 {
        return fmt_err("bad SuperBlob");
    }
    if blob.len() < 20 {
        return fmt_err("bad SuperBlob (no index entry)");
    }
    let o = be(&blob, 16) as usize;
    let cd = blob.get(o..).unwrap_or(&[]);
    if cd.len() < 40 {
        return fmt_err("CodeDirectory is shorter than its fixed header");
    }
    let v = CodeDirectoryView {
        flags: be(cd, 12),
        hash_offset: be(cd, 16),
        ident_offset: be(cd, 20),
        n_special: be(cd, 24),
        n_code: be(cd, 28),
        code_limit: be(cd, 32),
        hash_size: cd[36],
        hash_type: cd[37],
        page_log2: cd[39],
    };
    if v.page_log2 > 24 {
        return fmt_err(format!("implausible page size 1<<{}", v.page_log2));
    }
    let page = 1u64 << v.page_log2;
    // What a CodeDirectory must say for "every page matched" to mean anything. `hashSize == 0` makes `want == got[..0]` true for every slot, `nCodeSlots == 0` makes the loop empty (`0 == 0`), a count
    // smaller than `codeLimit` needs looks at the first pages only, one larger is walked slot by slot (2^32 - 1 of them: ~40 s here), and a hash type that is not SHA-256 was never compared as one.
    // Every signature this tool writes has hashSize 32, hashType 2 and nCodeSlots == ceil(codeLimit / pageSize); a bad one is not "verified", whatever the loop would have counted.
    // codeLimit and nCodeSlots are words of the file, not of its size: a codeLimit past the end of the file (nothing there to hash), or a hash
    // table that does not fit in the CodeDirectory, is not a signature of this file either. Both bound the walk by the file's own length (with
    // `page_log2 = 0` and codeLimit 0xFFFFFFFF, nCodeSlots == ceil(codeLimit / page) used to be 2^32 - 1 hashes of empty pages).
    let well_formed = v.hash_size == 32
        && v.hash_type == HASH_TYPE_SHA256
        && v.n_code != 0
        && u64::from(v.n_code) == u64::from(v.code_limit).div_ceil(page)
        && u64::from(v.code_limit) <= len
        && u64::from(v.hash_offset) + u64::from(v.n_code) * 32 <= cd.len() as u64;
    if !well_formed {
        return Ok(VerifyInfo {
            flag_adhoc: v.flags & CS_ADHOC != 0,
            n_code: v.n_code,
            pages_ok: 0,
            code_limit: v.code_limit,
            dataoff,
            matches: false,
        });
    }
    let hs = v.hash_size as usize;
    let n_code = v.n_code as usize;
    let chunk_slots = (CHUNK_PAGES * PAGE / page as usize).max(1);
    let n_chunks = n_code.div_ceil(chunk_slots);
    let counts: Vec<Result<u32, Error>> = (0..n_chunks)
        .into_par_iter()
        .map(|c| {
            let s0 = c * chunk_slots;
            let s1 = (s0 + chunk_slots).min(n_code);
            let lo = (s0 as u64 * page).min(len);
            let hi = (s1 as u64 * page).min(v.code_limit as u64).min(len).max(lo);
            let mut buf = vec![0u8; (hi - lo) as usize];
            src.read_at(lo, &mut buf)?;
            let mut ok = 0u32;
            for slot in s0..s1 {
                let a = (slot as u64 * page).min(len);
                let b = ((slot as u64 + 1) * page)
                    .min(v.code_limit as u64)
                    .min(len)
                    .max(a);
                let p = &buf[(a - lo) as usize..(b - lo) as usize];
                let w0 = (v.hash_offset as usize)
                    .saturating_add(slot * hs)
                    .min(cd.len());
                let w1 = (v.hash_offset as usize)
                    .saturating_add((slot + 1) * hs)
                    .min(cd.len());
                let want = &cd[w0..w1];
                let got = Sha256::digest(p);
                if want == &got[..hs.min(32)] {
                    ok += 1;
                }
            }
            Ok(ok)
        })
        .collect();
    let mut ok = 0u32;
    for c in counts {
        ok += c?;
    }
    Ok(VerifyInfo {
        flag_adhoc: v.flags & CS_ADHOC != 0,
        n_code: v.n_code,
        pages_ok: ok,
        code_limit: v.code_limit,
        dataoff,
        matches: ok == v.n_code,
    })
}

pub fn verify_bytes(data: &[u8]) -> Result<VerifyInfo, Error> {
    verify_source(data)
}

pub fn verify_file(path: &Path) -> Result<VerifyInfo, Error> {
    let file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    verify_source(&FileSource { file, len })
}
