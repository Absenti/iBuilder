//! `ibuilder_nup_icon` through the C ABI (raw pointers, a NUL-terminated path, a caller's buffer): the icon of a small synthetic Switch 2 package (written here: one base package, an
//! `exefs/` with `main` and `main.npdm`, a meta block), the buffer protocol (fits / too small / length only), no icon, a damaged or truncated head, NULL arguments.
//! The container writer is a small one; the nup crate's own tests (`crates/ibuilder-nup/tests/synthetic.rs`) cover the reader against containers of every shape.

use ibuilder_ffi::{
    E_BAD_ARG, E_NUP, ibuilder_last_error, ibuilder_nup_icon, ibuilder_nup_icon_read,
};
use sha2::{Digest, Sha256};
use std::ffi::{CString, c_char, c_int};
use std::path::{Path, PathBuf};

const APP: u64 = 0x0400_4860_24DE_C000;

fn npdm(program_id: u64) -> Vec<u8> {
    let mut v = vec![0u8; 0x200];
    v[..4].copy_from_slice(b"META");
    v[0x70..0x74].copy_from_slice(&0x100u32.to_le_bytes());
    v[0x74..0x78].copy_from_slice(&0x40u32.to_le_bytes());
    v[0x100..0x104].copy_from_slice(b"ACI0");
    v[0x110..0x118].copy_from_slice(&program_id.to_le_bytes());
    v
}

/// one base package: folders root(0) and exefs(1); files main (1 byte) and main.npdm; the meta block (TLV records) after them in the data blob
fn nup(meta: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let pool = b"exefsmainmain.npdm"; // root "" at 0 (empty), exefs 0..5, main 5..9, main.npdm 9..18
    let files: [(u32, u32, &[u8]); 2] = [(5, 4, b"m"), (9, 9, &[])];
    let npdm = npdm(APP);
    let mut block = vec![];
    for (t, d) in meta {
        block.extend_from_slice(&t.to_le_bytes());
        block.extend_from_slice(&0u16.to_le_bytes());
        block.extend_from_slice(&(d.len() as u32).to_le_bytes());
        block.extend_from_slice(d);
    }
    let (n_pkg, n_dirs, n_files, n_slots) = (1usize, 2usize, 2usize, 3usize);
    let need = 0x30 + n_pkg * 0x40 + n_dirs * 0x18 + n_files * 0x50;
    let pool_off = need + 4 * n_slots;
    let toc_size = pool_off + pool.len();
    let blob = (0x80 + toc_size + 15) & !15;
    // data blob: main at 0, main.npdm at 16, the meta block at 16 + 0x200
    let (main_off, npdm_off, meta_off) = (blob, blob + 16, blob + 16 + 0x200);
    let total = meta_off + block.len();
    let mut toc = vec![0u8; toc_size];
    toc[..4].copy_from_slice(b"TOC\x01");
    toc[4..8].copy_from_slice(&(n_pkg as u32).to_le_bytes());
    toc[8..16].copy_from_slice(&(n_dirs as u64).to_le_bytes());
    toc[16..24].copy_from_slice(&(n_files as u64).to_le_bytes());
    toc[24..32].copy_from_slice(&(pool_off as u64).to_le_bytes());
    toc[32..40].copy_from_slice(&(pool.len() as u64).to_le_bytes());
    let mut p = 0x30;
    toc[p..p + 8].copy_from_slice(&APP.to_le_bytes());
    toc[p + 8..p + 12].copy_from_slice(&0u32.to_le_bytes()); // root
    toc[p + 12..p + 16].copy_from_slice(&1u32.to_le_bytes()); // exefs
    toc[p + 16..p + 20].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // no romfs
    toc[p + 20..p + 24].copy_from_slice(&65536u32.to_le_bytes());
    let (mo, ms) = if block.is_empty() {
        (0u64, 0u32)
    } else {
        (meta_off as u64, block.len() as u32)
    };
    toc[p + 32..p + 40].copy_from_slice(&mo.to_le_bytes());
    toc[p + 40..p + 44].copy_from_slice(&ms.to_le_bytes());
    p += 0x40;
    // folders: name offset, length, first slot, count (root: slot 0, one child; exefs: slots 1, 2: its two files)
    for (o, l, first, count) in [(0u32, 0u32, 0u32, 1u32), (0, 5, 1, 2)] {
        toc[p..p + 4].copy_from_slice(&o.to_le_bytes());
        toc[p + 4..p + 8].copy_from_slice(&l.to_le_bytes());
        toc[p + 8..p + 12].copy_from_slice(&first.to_le_bytes());
        toc[p + 12..p + 16].copy_from_slice(&count.to_le_bytes());
        p += 0x18;
    }
    for (i, (o, l, data)) in files.iter().enumerate() {
        let data: &[u8] = if i == 1 { &npdm } else { data };
        toc[p..p + 4].copy_from_slice(&o.to_le_bytes());
        toc[p + 4..p + 8].copy_from_slice(&l.to_le_bytes());
        toc[p + 8..p + 16].copy_from_slice(&(data.len() as u64).to_le_bytes());
        let eoff = if i == 0 { main_off } else { npdm_off } as u64;
        toc[p + 16..p + 24].copy_from_slice(&eoff.to_le_bytes());
        toc[p + 24..p + 32].copy_from_slice(&(data.len() as u64).to_le_bytes());
        toc[p + 32..p + 64].copy_from_slice(&Sha256::digest(data));
        p += 0x50;
    }
    // slots: a slot is a folder's index, or the folder count plus a file's index
    for s in [1u32, 2, 3] {
        toc[p..p + 4].copy_from_slice(&s.to_le_bytes());
        p += 4;
    }
    toc[p..].copy_from_slice(pool);
    let mut out = vec![0u8; 0x80];
    out[..4].copy_from_slice(b"NUP\x01");
    out[4..6].copy_from_slice(&0x80u16.to_le_bytes());
    out[8..16].copy_from_slice(&0x80u64.to_le_bytes());
    out[16..24].copy_from_slice(&(toc_size as u64).to_le_bytes());
    out[24..56].copy_from_slice(&Sha256::digest(&toc));
    out[56..64].copy_from_slice(&(blob as u64).to_le_bytes());
    out[64..72].copy_from_slice(&(total as u64).to_le_bytes());
    out[72..76].copy_from_slice(&(n_pkg as u32).to_le_bytes());
    out.extend_from_slice(&toc);
    out.resize(blob, 0);
    out.extend_from_slice(b"m");
    out.resize(npdm_off, 0);
    out.extend_from_slice(&npdm);
    out.resize(meta_off, 0);
    out.extend_from_slice(&block);
    assert_eq!(out.len(), total);
    out
}

fn tmp(tag: &str) -> PathBuf {
    let base = std::env::var("IBUILDER_TEST_TMP")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let d = base.join(format!(
        "ibuilder-ffi-nup-icon-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> CString {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    CString::new(p.to_str().unwrap()).unwrap()
}

/// (result, the bytes of the buffer after the call, *title_id)
fn call(path: &CString, cap: usize) -> (c_int, Vec<u8>, u64) {
    let mut buf = vec![0xEEu8; cap];
    let mut title = 0xDEAD_BEEFu64;
    // SAFETY: the path is NUL-terminated, the buffer is `cap` bytes, the title is one u64.
    let r = unsafe { ibuilder_nup_icon(path.as_ptr(), buf.as_mut_ptr(), cap as c_int, &mut title) };
    (r, buf, title)
}

fn last_error() -> String {
    let mut b = vec![0 as c_char; 512];
    // SAFETY: 512 writable bytes.
    let n = unsafe { ibuilder_last_error(b.as_mut_ptr(), 512) };
    let bytes: Vec<u8> = b
        .iter()
        .take((n as usize).min(511))
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn the_icon_and_the_title_come_through_the_c_abi_and_the_buffer_protocol_is_the_documented_one() {
    let d = tmp("icon");
    let icon: Vec<u8> = (0..3000u32).map(|i| (i * 7 + 3) as u8).collect();
    let path = write(
        &d,
        "game.nup",
        &nup(&[(2, vec![0; 0x4000]), (3, icon.clone())]),
    );
    // it fits: the length, the bytes, the title
    let (r, buf, title) = call(&path, 4096);
    assert_eq!((r, &buf[..3000], title), (3000, &icon[..], APP));
    assert!(
        buf[3000..].iter().all(|b| *b == 0xEE),
        "bytes written past the icon"
    );
    // exactly its size
    let (r, buf, _) = call(&path, 3000);
    assert_eq!((r, &buf[..]), (3000, &icon[..]));
    // too small: the length it needs, nothing written, the title still given
    let (r, buf, title) = call(&path, 100);
    assert_eq!((r, title), (3000, APP));
    assert!(buf.iter().all(|b| *b == 0xEE));
    // the length only: no buffer
    let mut title = 0u64;
    // SAFETY: a NULL buffer with cap 0 is the documented length query.
    let r = unsafe { ibuilder_nup_icon(path.as_ptr(), std::ptr::null_mut(), 0, &mut title) };
    assert_eq!((r, title), (3000, APP));
    // title_id may be NULL
    let mut b = [0u8; 4096];
    // SAFETY: documented: title_id may be NULL.
    assert_eq!(
        unsafe { ibuilder_nup_icon(path.as_ptr(), b.as_mut_ptr(), 4096, std::ptr::null_mut()) },
        3000
    );
    // localized icons only: the en-US one
    let mut set = vec![];
    for (tag, data) in [("fr", &b"FRENCH"[..]), ("en-US", &b"ENGLISH"[..])] {
        set.push(tag.len() as u8);
        set.extend_from_slice(tag.as_bytes());
        set.extend_from_slice(&(data.len() as u32).to_le_bytes());
        set.extend_from_slice(data);
    }
    let path = write(&d, "loc.nup", &nup(&[(10, set)]));
    let (r, buf, title) = call(&path, 64);
    assert_eq!((r, &buf[..7], title), (7, &b"ENGLISH"[..], APP));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_package_with_no_icon_answers_zero_and_still_names_the_title() {
    let d = tmp("none");
    // the real Ocarina package: the NPDM and three small records, no NACP, no icon
    let path = write(
        &d,
        "none.nup",
        &nup(&[
            (1, npdm(APP)),
            (5, vec![0; 36]),
            (7, vec![0; 32]),
            (8, vec![0; 4]),
        ]),
    );
    let (r, buf, title) = call(&path, 64);
    assert_eq!((r, title), (0, APP));
    assert!(buf.iter().all(|b| *b == 0xEE));
    let path = write(&d, "nometa.nup", &nup(&[]));
    assert_eq!(call(&path, 64).0, 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_damaged_truncated_or_missing_file_is_an_error_code_with_a_message_never_a_crash() {
    let d = tmp("bad");
    let good = nup(&[(3, vec![1, 2, 3, 4])]);
    assert_eq!(call(&write(&d, "good.nup", &good), 16).0, 4);
    // cut at every length: a header that says more than the file holds
    for len in [0, 1, 3, 0x7f, 0x80, 0x90, 0x100, good.len() - 1] {
        let path = write(&d, "cut.nup", &good[..len]);
        let (r, _, title) = call(&path, 16);
        assert_eq!((r, title), (E_NUP, 0), "cut at {len}");
        assert!(!last_error().is_empty(), "cut at {len}: no message");
    }
    // a byte of the table of contents flipped: its checksum says so
    let mut bad = good.clone();
    bad[0x90] ^= 0xFF;
    let (r, _, _) = call(&write(&d, "flip.nup", &bad), 16);
    assert_eq!(r, E_NUP);
    assert!(last_error().contains("SHA-256"), "{}", last_error());
    // not a NUP at all, and a file that is not there
    assert_eq!(call(&write(&d, "text.nup", b"this is not a switch 2 package, only text of a certain length........................................................................................................."), 16).0, E_NUP);
    let missing = CString::new(d.join("missing.nup").to_str().unwrap()).unwrap();
    assert_eq!(call(&missing, 16).0, E_NUP);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn bad_arguments_are_refused_before_anything_is_read() {
    let d = tmp("args");
    let path = write(&d, "g.nup", &nup(&[(3, vec![9; 10])]));
    let mut b = [0u8; 16];
    let mut title = 7u64;
    // SAFETY (all calls): each pointer is NULL or valid for what is passed.
    unsafe {
        assert_eq!(
            ibuilder_nup_icon(std::ptr::null(), b.as_mut_ptr(), 16, &mut title),
            E_BAD_ARG
        );
        assert_eq!(title, 0, "the title is cleared first");
        assert_eq!(
            ibuilder_nup_icon(path.as_ptr(), b.as_mut_ptr(), -1, &mut title),
            E_BAD_ARG
        );
        assert_eq!(
            ibuilder_nup_icon(path.as_ptr(), std::ptr::null_mut(), 16, &mut title),
            E_BAD_ARG
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

// ---- ibuilder_nup_icon_read (app patch 0084: a .nup on the NAS, read through the caller's callback) ----------------------------------------------------------------------------------

struct Remote {
    bytes: Vec<u8>,
    reads: std::cell::Cell<u64>,
    fail_after: Option<u64>,
}

unsafe extern "C" fn remote_read(
    ctx: *mut std::ffi::c_void,
    offset: u64,
    buf: *mut u8,
    len: c_int,
) -> i64 {
    // SAFETY: the test passes a live `Remote` as ctx and the library a buffer of `len` bytes.
    let r = unsafe { &*(ctx as *const Remote) };
    r.reads.set(r.reads.get() + 1);
    if r.fail_after.is_some_and(|n| r.reads.get() > n) {
        return -5;
    }
    let off = offset as usize;
    if off >= r.bytes.len() {
        return 0;
    }
    let n = (len as usize).min(r.bytes.len() - off);
    // SAFETY: `n <= len` bytes into the caller's buffer.
    unsafe { std::ptr::copy_nonoverlapping(r.bytes[off..].as_ptr(), buf, n) };
    n as i64
}

fn call_read(r: &Remote, size: u64, cap: usize) -> (c_int, Vec<u8>, u64) {
    let mut buf = vec![0xEEu8; cap];
    let mut title = 0xDEAD_BEEFu64;
    // SAFETY: a live Remote, a buffer of cap bytes, one u64.
    let rc = unsafe {
        ibuilder_nup_icon_read(
            Some(remote_read),
            r as *const Remote as *mut std::ffi::c_void,
            size,
            buf.as_mut_ptr(),
            cap as c_int,
            &mut title,
        )
    };
    (rc, buf, title)
}

#[test]
fn a_remote_package_read_through_the_callback_gives_the_same_icon_and_title_as_the_file() {
    let icon: Vec<u8> = (0..3000u32).map(|i| (i * 7 + 3) as u8).collect();
    let bytes = nup(&[(2, vec![0; 0x4000]), (3, icon.clone())]);
    let r = Remote {
        bytes: bytes.clone(),
        reads: 0.into(),
        fail_after: None,
    };
    let (rc, buf, title) = call_read(&r, bytes.len() as u64, 4096);
    assert_eq!((rc, &buf[..3000], title), (3000, &icon[..], APP));
    assert!(r.reads.get() > 0);
    // the same through the file: one answer
    let d = tmp("remote");
    let path = write(&d, "same.nup", &bytes);
    let (rf, bf, tf) = call(&path, 4096);
    assert_eq!((rf, &bf[..3000], tf), (rc, &buf[..3000], title));
    // too small a buffer: the length, nothing written
    let (rc, buf, _) = call_read(&r, bytes.len() as u64, 10);
    assert_eq!(rc, 3000);
    assert!(buf.iter().all(|b| *b == 0xEE));
    // no icon (Ocarina's case): 0 and the title
    let none = nup(&[(2, vec![0; 64])]);
    let r = Remote {
        bytes: none.clone(),
        reads: 0.into(),
        fail_after: None,
    };
    let (rc, _, title) = call_read(&r, none.len() as u64, 64);
    assert_eq!((rc, title), (0, APP));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_failing_or_short_reader_is_an_error_never_a_crash() {
    let bytes = nup(&[(3, vec![1, 2, 3])]);
    // the reader fails after its first read
    let r = Remote {
        bytes: bytes.clone(),
        reads: 0.into(),
        fail_after: Some(1),
    };
    let (rc, _, title) = call_read(&r, bytes.len() as u64, 64);
    assert_eq!((rc, title), (E_NUP, 0));
    assert!(!last_error().is_empty());
    // a size that claims more than the reader has (a truncated remote file)
    let r = Remote {
        bytes: bytes[..200].to_vec(),
        reads: 0.into(),
        fail_after: None,
    };
    let (rc, _, _) = call_read(&r, bytes.len() as u64, 64);
    assert_eq!(rc, E_NUP);
    // not a package
    let r = Remote {
        bytes: vec![0u8; 4096],
        reads: 0.into(),
        fail_after: None,
    };
    assert_eq!(call_read(&r, 4096, 64).0, E_NUP);
    // no reader, a negative cap
    let mut b = [0u8; 8];
    // SAFETY: a NULL reader is a documented bad argument.
    assert_eq!(
        unsafe {
            ibuilder_nup_icon_read(
                None,
                std::ptr::null_mut(),
                10,
                b.as_mut_ptr(),
                8,
                std::ptr::null_mut(),
            )
        },
        E_BAD_ARG
    );
    let r = Remote {
        bytes: bytes.clone(),
        reads: 0.into(),
        fail_after: None,
    };
    // SAFETY: a negative cap is a documented bad argument.
    assert_eq!(
        unsafe {
            ibuilder_nup_icon_read(
                Some(remote_read),
                &r as *const Remote as *mut std::ffi::c_void,
                bytes.len() as u64,
                b.as_mut_ptr(),
                -1,
                std::ptr::null_mut(),
            )
        },
        E_BAD_ARG
    );
}
