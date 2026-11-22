//! Unit tests on a hand-made Mach-O (no toolchain needed): format of the blob, idempotence, error paths, and that
//! the in-memory and in-place (streamed) signers agree.
use ibuilder_sign::{
    AdhocSigner, CodeSigner, SignOptions, SignTarget, sign_bytes, sign_bytes_with, sign_file,
    verify_bytes, verify_file,
};
use sha2::{Digest, Sha256};

fn p32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn p64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn name16(s: &str) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[..s.len()].copy_from_slice(s.as_bytes());
    b
}

/// A minimal arm64 dylib: __TEXT (one section at file offset `sect_off`), __LINKEDIT, LC_ID_DYLIB `id`.
fn macho(id: Option<&str>, sect_off: u32, with_linkedit: bool) -> Vec<u8> {
    let mut cmds: Vec<u8> = Vec::new();
    let mut ncmds = 0;
    // __TEXT, one section
    p32(&mut cmds, 0x19);
    p32(&mut cmds, 72 + 80);
    cmds.extend_from_slice(&name16("__TEXT"));
    for x in [0u64, 0x4000, 0, 0x2000] {
        p64(&mut cmds, x);
    }
    p32(&mut cmds, 5);
    p32(&mut cmds, 5);
    p32(&mut cmds, 1);
    p32(&mut cmds, 0);
    cmds.extend_from_slice(&name16("__text"));
    cmds.extend_from_slice(&name16("__TEXT"));
    p64(&mut cmds, 0x1000); // addr
    p64(&mut cmds, 0x10); // size
    p32(&mut cmds, sect_off);
    for _ in 0..7 {
        p32(&mut cmds, 0);
    }
    ncmds += 1;
    if with_linkedit {
        p32(&mut cmds, 0x19);
        p32(&mut cmds, 72);
        cmds.extend_from_slice(&name16("__LINKEDIT"));
        for x in [0x4000u64, 0x4000, 0x2000, 0x100] {
            p64(&mut cmds, x);
        }
        for _ in 0..4 {
            p32(&mut cmds, 0);
        }
        ncmds += 1;
    }
    if let Some(id) = id {
        let mut n = id.as_bytes().to_vec();
        n.push(0);
        while n.len() % 8 != 0 {
            n.push(0);
        }
        p32(&mut cmds, 0xD);
        p32(&mut cmds, 24 + n.len() as u32);
        p32(&mut cmds, 24);
        p32(&mut cmds, 0);
        p32(&mut cmds, 0x10000);
        p32(&mut cmds, 0x10000);
        cmds.extend_from_slice(&n);
        ncmds += 1;
    }
    let mut f = Vec::new();
    p32(&mut f, 0xFEED_FACF);
    p32(&mut f, 0x0100_000C);
    p32(&mut f, 0);
    p32(&mut f, 6);
    p32(&mut f, ncmds);
    p32(&mut f, cmds.len() as u32);
    p32(&mut f, 0x0010_0085);
    p32(&mut f, 0);
    f.extend_from_slice(&cmds);
    f.resize(0x2100, 0);
    for (i, b) in f.iter_mut().enumerate().skip(sect_off as usize) {
        *b = (i * 7 + 3) as u8;
    }
    f
}

fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(d[o..o + 4].try_into().unwrap())
}
fn le32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}
fn le64(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}

#[test]
fn signature_layout_matches_adhoc_sign_py() {
    let mut f = macho(Some("@rpath/X.framework/X"), 0x1000, true);
    let orig_len = f.len();
    let info = sign_bytes(&mut f, None, "fallback").unwrap();
    assert_eq!(info.identifier, "@rpath/X.framework/X");
    assert_eq!(info.code_limit as usize, orig_len); // 0x2100 is 16-aligned already
    assert_eq!(info.n_code, 3);
    let name_len = "@rpath/X.framework/X".len() + 1;
    let cd_len = 88 + name_len + 3 * 32;
    assert_eq!(info.datasize as usize, (20 + cd_len + 7) & !7);
    assert_eq!(f.len(), orig_len + info.datasize as usize);
    // header: ncmds+1, sizeofcmds+16, LC_CODE_SIGNATURE written in the slack
    let (ncmds, socmds) = (le32(&f, 16), le32(&f, 20));
    let lc = 32 + socmds as usize - 16;
    assert_eq!((le32(&f, lc), le32(&f, lc + 4)), (0x1d, 16));
    assert_eq!(
        (le32(&f, lc + 8), le32(&f, lc + 12)),
        (orig_len as u32, info.datasize)
    );
    assert_eq!(ncmds, 4);
    // __LINKEDIT grown to hold the blob: filesize = code_limit + super_len - fileoff, vmsize aligned up
    let le = 32 + 152; // second command
    assert_eq!(
        le64(&f, le + 48),
        orig_len as u64 + info.datasize as u64 - 0x2000
    );
    assert!(le64(&f, le + 32) >= le64(&f, le + 48));
    // SuperBlob + CodeDirectory fields (big-endian)
    let b = &f[orig_len..];
    assert_eq!(
        (be32(b, 0), be32(b, 8), be32(b, 12), be32(b, 16)),
        (0xFADE_0CC0, 1, 0, 20)
    );
    assert_eq!(be32(b, 4) as usize, 20 + cd_len);
    let cd = &b[20..];
    assert_eq!(be32(cd, 0), 0xFADE_0C02);
    assert_eq!(be32(cd, 4) as usize, cd_len);
    assert_eq!((be32(cd, 8), be32(cd, 12)), (0x20400, 2));
    assert_eq!(
        (
            be32(cd, 16) as usize,
            be32(cd, 20),
            be32(cd, 24),
            be32(cd, 28),
            be32(cd, 32)
        ),
        (88 + name_len, 88, 0, 3, orig_len as u32)
    );
    assert_eq!(&cd[36..40], &[32, 2, 0, 12]);
    assert!(cd[40..88].iter().all(|&x| x == 0));
    assert_eq!(&cd[88..88 + name_len - 1], b"@rpath/X.framework/X");
    // every page hash = SHA-256 of the page, last one unpadded
    for slot in 0..3 {
        let page = &f[slot * 4096..((slot + 1) * 4096).min(orig_len)];
        let h = &cd[88 + name_len + slot * 32..88 + name_len + (slot + 1) * 32];
        assert_eq!(h, Sha256::digest(page).as_slice(), "slot {slot}");
    }
    let v = verify_bytes(&f).unwrap();
    assert!(
        v.matches && v.flag_adhoc && v.n_code == 3 && v.pages_ok == 3,
        "{v:?}"
    );
}

#[test]
fn resigning_is_idempotent_and_drops_the_old_blob() {
    let mut f = macho(Some("id"), 0x1000, true);
    sign_bytes(&mut f, None, "x").unwrap();
    let once = f.clone();
    sign_bytes(&mut f, None, "x").unwrap();
    assert_eq!(
        f, once,
        "a second run must reproduce the same file (blobs do not stack)"
    );
    // a different identifier replaces the first one
    sign_bytes(&mut f, Some("other.id"), "x").unwrap();
    assert_ne!(f, once);
    assert!(verify_bytes(&f).unwrap().matches);
}

#[test]
fn identifier_precedence_is_arg_then_lc_id_dylib_then_default() {
    let mut a = macho(Some("from.lc"), 0x1000, true);
    assert_eq!(
        sign_bytes(&mut a, Some("arg"), "base").unwrap().identifier,
        "arg"
    );
    let mut a = macho(Some("from.lc"), 0x1000, true);
    assert_eq!(
        sign_bytes(&mut a, Some(""), "base").unwrap().identifier,
        "from.lc"
    ); // empty is falsy in the Python
    let mut a = macho(None, 0x1000, true);
    assert_eq!(sign_bytes(&mut a, None, "base").unwrap().identifier, "base");
}

#[test]
fn unaligned_length_is_zero_padded_into_the_signed_region() {
    let mut f = macho(Some("id"), 0x1000, true);
    f.extend_from_slice(&[1, 2, 3]); // 0x2103: codeLimit = 0x2110
    let info = sign_bytes(&mut f, None, "x").unwrap();
    assert_eq!(info.code_limit, 0x2110);
    assert!(f[0x2103..0x2110].iter().all(|&b| b == 0));
    assert!(verify_bytes(&f).unwrap().matches);
}

#[test]
fn tampering_is_detected() {
    let mut f = macho(Some("id"), 0x1000, true);
    sign_bytes(&mut f, None, "x").unwrap();
    f[0x1008] ^= 0xff;
    let v = verify_bytes(&f).unwrap();
    assert!(!v.matches);
    assert_eq!(v.pages_ok, v.n_code - 1);
}

#[test]
fn errors_match_the_python_messages() {
    let mut f = macho(Some("id"), 0x1000, false);
    assert_eq!(
        sign_bytes(&mut f, None, "x").unwrap_err().to_string(),
        "no __LINKEDIT segment: nothing to grow for the signature"
    );
    // a section right behind the load commands leaves no room for the 16-byte LC_CODE_SIGNATURE
    let mut f = macho(Some("id"), 32 + 152 + 72 + 32 + 8, true);
    assert_eq!(
        sign_bytes(&mut f, None, "x").unwrap_err().to_string(),
        "no room for an LC_CODE_SIGNATURE command (headerpad too small)"
    );
    let mut f = vec![0u8; 64];
    assert!(
        sign_bytes(&mut f, None, "x")
            .unwrap_err()
            .to_string()
            .starts_with("not a 64-bit little-endian Mach-O")
    );
    assert!(
        verify_bytes(&macho(Some("id"), 0x1000, true))
            .unwrap_err()
            .to_string()
            .contains("no LC_CODE_SIGNATURE")
    );
}

#[test]
fn file_signer_streams_and_equals_the_in_memory_signer() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(&dir).unwrap();
    // large enough for several rayon chunks (CHUNK_PAGES = 256 pages = 1 MiB): ~5 MiB of pseudo-random tail
    let mut f = macho(Some("id"), 0x1000, true);
    let mut x = 0x1234_5678_9abc_def0u64;
    f.resize(5 * 1024 * 1024 + 37, 0);
    for b in f.iter_mut().skip(0x2100) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
    let mut mem = f.clone();
    let mi = sign_bytes(&mut mem, Some("rs.test"), "x").unwrap();
    let path = dir.join("unit-stream.dylib");
    std::fs::write(&path, &f).unwrap();
    let fi = sign_file(&path, Some("rs.test")).unwrap();
    assert_eq!(mi, fi);
    assert_eq!(std::fs::read(&path).unwrap(), mem);
    let v = verify_file(&path).unwrap();
    assert!(v.matches, "{v:?}");
    assert_eq!(v.n_code, mi.n_code as u32);
}

#[test]
fn shrink_option_fixes_linkedit_when_a_larger_signature_is_replaced() {
    let mut f = macho(
        Some(
            "a-rather-long-identifier-making-the-first-blob-bigger-than-the-second/xxxxxxxxxxxxxxxxxxxxxx",
        ),
        0x1000,
        true,
    );
    sign_bytes(&mut f, None, "x").unwrap();
    let big = f.clone();
    // re-sign with a short identifier: the signature is smaller than the one it replaces
    let mut default = big.clone();
    let a = sign_bytes(&mut default, Some("s"), "x").unwrap();
    let mut shrunk = big.clone();
    let b = sign_bytes_with(
        &mut shrunk,
        Some("s"),
        "x",
        &SignOptions {
            shrink_linkedit: true,
        },
    )
    .unwrap();
    assert_eq!(a, b);
    let le = 32 + 152;
    let end = |d: &[u8]| le64(d, le + 40) + le64(d, le + 48);
    assert!(
        end(&default) > default.len() as u64,
        "python behaviour: __LINKEDIT reaches past EOF"
    );
    assert_eq!(
        end(&shrunk),
        shrunk.len() as u64,
        "with the option it ends exactly at EOF"
    );
    assert!(verify_bytes(&shrunk).unwrap().matches);
}

#[test]
fn adhoc_signer_trait_signs_a_file_in_place() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("unit-trait.dylib");
    std::fs::write(&path, macho(Some("x"), 0x1000, true)).unwrap();
    let signer: Box<dyn CodeSigner> = Box::new(AdhocSigner::default());
    let r = signer
        .sign(&SignTarget {
            framework: None,
            executable: &path,
            identifier: Some("dev.test.app"),
        })
        .unwrap();
    assert_eq!(r.identifier, "dev.test.app");
    assert!(r.mode.starts_with("adhoc("));
    assert!(verify_file(&path).unwrap().matches);
}

// ---- review (deepseek 4.1 / r2): `verify` must not be able to answer `matches` without having hashed what the CodeDirectory says it covers ----------------------------------------------

/// Sign a small image, then overwrite one byte-field of its CodeDirectory (offsets from the CodeDirectory's start: nCodeSlots 28, hashSize 36, hashType 37)
fn signed_with_cd(patch: impl Fn(&mut [u8])) -> Vec<u8> {
    let mut f = macho(Some("id"), 0x1000, true);
    f.resize(0x2100, 0x5a); // three pages, so n_code = 3
    let info = sign_bytes(&mut f, None, "x").unwrap();
    let cd = info.code_limit as usize + 20;
    patch(&mut f[cd..]);
    f
}

#[test]
fn a_codedirectory_that_hashes_nothing_does_not_verify() {
    assert!(
        verify_bytes(&signed_with_cd(|_| {})).unwrap().matches,
        "control: the untouched signature verifies"
    );
    // hashSize 0: every `want == got[..0]` is true for every page
    let v = verify_bytes(&signed_with_cd(|cd| cd[36] = 0)).unwrap();
    assert!(!v.matches, "hashSize 0 must not verify: {v:?}");
    // nCodeSlots 0: the loop never runs and `ok == n_code` is `0 == 0`
    let v = verify_bytes(&signed_with_cd(|cd| {
        cd[28..32].copy_from_slice(&0u32.to_be_bytes())
    }))
    .unwrap();
    assert!(!v.matches, "nCodeSlots 0 must not verify: {v:?}");
    // nCodeSlots 1 of the 3 pages codeLimit covers: only the first page is looked at
    let v = verify_bytes(&signed_with_cd(|cd| {
        cd[28..32].copy_from_slice(&1u32.to_be_bytes())
    }))
    .unwrap();
    assert!(!v.matches, "nCodeSlots 1 of 3 pages must not verify: {v:?}");
    // a hash type that is not SHA-256 (2) is not what the 32 bytes were compared as
    let v = verify_bytes(&signed_with_cd(|cd| cd[37] = 1)).unwrap();
    assert!(
        !v.matches,
        "hashType 1 (SHA-1) must not verify as SHA-256: {v:?}"
    );
}

#[test]
fn a_huge_ncodeslots_is_refused_at_once_not_looped_over() {
    // codeLimit covers 3 pages; a CodeDirectory that claims 2^32 - 1 of them used to be walked slot by slot
    let t = std::time::Instant::now();
    let v = verify_bytes(&signed_with_cd(|cd| {
        cd[28..32].copy_from_slice(&u32::MAX.to_be_bytes())
    }))
    .unwrap();
    assert!(!v.matches, "{v:?}");
    assert!(t.elapsed().as_secs() < 5, "took {:?}", t.elapsed());
}

#[test]
fn a_linkedit_that_claims_the_whole_address_space_is_an_error_not_a_wrapped_size() {
    // __TEXT is 32 + 152 bytes in; __LINKEDIT's filesize is at +48 of its command
    let mut f = macho(Some("id"), 0x1000, true);
    let le = 32 + 72 + 80;
    f[le + 48..le + 56].copy_from_slice(&(u64::MAX - 0xf).to_le_bytes());
    let e = sign_bytes(&mut f, None, "x").unwrap_err().to_string();
    assert!(e.contains("__LINKEDIT") && e.contains("overflow"), "{e}");
}

// ---- review 2026-11-20 §4.11: nCodeSlots follows codeLimit, a word of the file: bounded by the file's length ------------------------------------------------------------------------------

#[test]
fn a_codelimit_past_the_file_with_one_byte_pages_is_refused_at_once() {
    // pageSize 1 (page_log2 0), codeLimit 0xFFFFFFFF and nCodeSlots = ceil(codeLimit / 1): consistent with each other, so the old check walked 2^32 - 1 slots
    let t = std::time::Instant::now();
    let v = verify_bytes(&signed_with_cd(|cd| {
        cd[39] = 0;
        cd[32..36].copy_from_slice(&u32::MAX.to_be_bytes());
        cd[28..32].copy_from_slice(&u32::MAX.to_be_bytes());
    }))
    .unwrap();
    assert!(!v.matches, "{v:?}");
    assert_eq!(v.pages_ok, 0, "{v:?}");
    assert!(t.elapsed().as_secs() < 5, "took {:?}", t.elapsed());
    // a codeLimit inside the file with a hash table that runs past the CodeDirectory: not a signature either
    let v = verify_bytes(&signed_with_cd(|cd| {
        cd[16..20].copy_from_slice(&0x7fff_0000u32.to_be_bytes());
    }))
    .unwrap();
    assert!(!v.matches, "{v:?}");
    assert_eq!(v.pages_ok, 0, "{v:?}");
}
