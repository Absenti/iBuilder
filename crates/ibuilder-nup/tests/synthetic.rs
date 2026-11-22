//! The NUP reader against synthetic containers written by this file (the format's v1: superblock, TOC with packages / folders / files / child slots / name pool, a 16-aligned data
//! blob, files raw or as one zstd frame, a SHA-256 each). No game data: the contents are generated.

use ibuilder_nup::romfs::read_index;
use ibuilder_nup::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---- a writer of containers -----------------------------------------------------------------------------------------------------

#[derive(Clone)]
enum Node {
    File(String, Vec<u8>, u8),
    Dir(String, Vec<Node>),
}

fn file(n: &str, d: &[u8]) -> Node {
    Node::File(n.into(), d.to_vec(), 0)
}
fn zfile(n: &str, d: &[u8]) -> Node {
    Node::File(n.into(), d.to_vec(), 1)
}
fn dir(n: &str, kids: Vec<Node>) -> Node {
    Node::Dir(n.into(), kids)
}

struct Pkg {
    title_id: u64,
    kind: u8,
    version: u32,
    flags: u8,
    meta: Vec<(u16, Vec<u8>)>,
    exefs: Option<Vec<Node>>,
    romfs: Option<Vec<Node>>,
}

fn npdm(program_id: u64) -> Vec<u8> {
    let mut v = vec![0u8; 0x200];
    v[..4].copy_from_slice(b"META");
    v[0x70..0x74].copy_from_slice(&0x100u32.to_le_bytes());
    v[0x74..0x78].copy_from_slice(&0x40u32.to_le_bytes());
    v[0x100..0x104].copy_from_slice(b"ACI0");
    v[0x110..0x118].copy_from_slice(&program_id.to_le_bytes());
    v
}

fn nacp(owner: u64, aoc_base: u64) -> Vec<u8> {
    let mut v = vec![0u8; 0x4000];
    v[0x3070..0x3078].copy_from_slice(&aoc_base.to_le_bytes());
    v[0x3078..0x3080].copy_from_slice(&owner.to_le_bytes());
    v
}

/// One file entry of the table of contents.
type TocFile = (u32, u32, u64, u64, u64, [u8; 32], u8);

#[derive(Default)]
struct Toc {
    dirs: Vec<(u32, u32, u32, u32)>, // name off, len, first slot, count
    files: Vec<TocFile>,
    slots: Vec<u32>,
    pool: Vec<u8>,
    blob: Vec<u8>,                          // data, relative to the blob start
    frames: BTreeMap<[u8; 32], (u64, u64)>, // dedup: sha -> (offset in blob, extent size) like a real packer's shared extents
}

fn pack_tree(t: &mut Toc, name: &str, kids: &[Node], file_base: usize) -> u32 {
    // a folder's children are consecutive slots: files/folders numbered after the folders' count is known, so folders are reserved first
    let idx = t.dirs.len() as u32;
    let off = t.pool.len() as u32;
    t.pool.extend_from_slice(name.as_bytes());
    t.dirs.push((off, name.len() as u32, 0, 0));
    let mut child_slots: Vec<(bool, u32)> = vec![];
    for k in kids {
        match k {
            Node::Dir(n, ks) => {
                let d = pack_tree(t, n, ks, file_base);
                child_slots.push((true, d));
            }
            Node::File(n, data, codec) => {
                let off = t.pool.len() as u32;
                t.pool.extend_from_slice(n.as_bytes());
                let sha: [u8; 32] = Sha256::digest(data).into();
                let (eoff, esz, c) = if data.is_empty() {
                    (0, 0, 0)
                } else if let Some(&(o, s)) = t.frames.get(&sha).filter(|_| true) {
                    (o, s, *codec)
                } else {
                    while !t.blob.len().is_multiple_of(16) {
                        t.blob.push(0);
                    }
                    let stored = if *codec == 1 {
                        zstd::bulk::compress(data, 3).unwrap()
                    } else {
                        data.clone()
                    };
                    let o = t.blob.len() as u64;
                    t.blob.extend_from_slice(&stored);
                    t.frames.insert(sha, (o, stored.len() as u64));
                    (o, stored.len() as u64, *codec)
                };
                let fi = t.files.len() as u32;
                t.files
                    .push((off, n.len() as u32, data.len() as u64, eoff, esz, sha, c));
                child_slots.push((false, fi));
            }
        }
    }
    let first = t.slots.len() as u32;
    // folder slots are their index, file slots are (number of folders) + file index: the number of folders is not final yet, so files are marked and fixed at the end
    for (is_dir, i) in &child_slots {
        t.slots.push(if *is_dir {
            *i
        } else {
            0x8000_0000 | (*i + file_base as u32)
        });
    }
    t.dirs[idx as usize].2 = first;
    t.dirs[idx as usize].3 = child_slots.len() as u32;
    idx
}

fn build(pkgs: &[Pkg], total_override: Option<u64>) -> Vec<u8> {
    let mut t = Toc::default();
    let mut recs: Vec<(u32, u32, u32)> = vec![];
    let mut metas: Vec<Vec<u8>> = vec![];
    for p in pkgs {
        let mut kids = vec![];
        if let Some(e) = &p.exefs {
            kids.push(dir("exefs", e.clone()));
        }
        if let Some(r) = &p.romfs {
            kids.push(dir("romfs", r.clone()));
        }
        // root first (index = its number), then the packages' own folders
        let root = pack_tree(&mut t, "", &kids, 0);
        let find = |n: &str| -> u32 {
            let d = t.dirs[root as usize];
            for s in &t.slots[d.2 as usize..(d.2 + d.3) as usize] {
                if *s & 0x8000_0000 == 0 {
                    let (o, l, _, _) = t.dirs[*s as usize];
                    if &t.pool[o as usize..(o + l) as usize] == n.as_bytes() {
                        return *s;
                    }
                }
            }
            0xFFFF_FFFF
        };
        recs.push((
            root,
            if p.exefs.is_some() {
                find("exefs")
            } else {
                0xFFFF_FFFF
            },
            if p.romfs.is_some() {
                find("romfs")
            } else {
                0xFFFF_FFFF
            },
        ));
        let mut m = vec![];
        for (ty, data) in &p.meta {
            m.extend_from_slice(&ty.to_le_bytes());
            m.extend_from_slice(&0u16.to_le_bytes());
            m.extend_from_slice(&(data.len() as u32).to_le_bytes());
            m.extend_from_slice(data);
        }
        metas.push(m);
    }
    let ndirs = t.dirs.len() as u32;
    for s in t.slots.iter_mut() {
        if *s & 0x8000_0000 != 0 {
            *s = (*s & 0x7fff_ffff) + ndirs;
        }
    }
    // meta blocks go in the blob too
    let mut meta_off = vec![];
    for m in &metas {
        while t.blob.len() % 16 != 0 {
            t.blob.push(0);
        }
        meta_off.push(t.blob.len() as u64);
        t.blob.extend_from_slice(m);
    }
    let npkg = pkgs.len();
    let need = 0x30 + npkg * 0x40 + t.dirs.len() * 0x18 + t.files.len() * 0x50;
    let pool_off = need + 4 * t.slots.len();
    let toc_size = pool_off + t.pool.len();
    let blob_off = (0x80 + toc_size + 15) & !15;
    let total = blob_off as u64 + t.blob.len() as u64;
    let mut toc = vec![0u8; toc_size];
    toc[..4].copy_from_slice(b"TOC\x01");
    toc[4..8].copy_from_slice(&(npkg as u32).to_le_bytes());
    toc[8..16].copy_from_slice(&(t.dirs.len() as u64).to_le_bytes());
    toc[16..24].copy_from_slice(&(t.files.len() as u64).to_le_bytes());
    toc[24..32].copy_from_slice(&(pool_off as u64).to_le_bytes());
    toc[32..40].copy_from_slice(&(t.pool.len() as u64).to_le_bytes());
    let mut p = 0x30;
    for (i, pk) in pkgs.iter().enumerate() {
        toc[p..p + 8].copy_from_slice(&pk.title_id.to_le_bytes());
        toc[p + 8..p + 12].copy_from_slice(&recs[i].0.to_le_bytes());
        toc[p + 12..p + 16].copy_from_slice(&recs[i].1.to_le_bytes());
        toc[p + 16..p + 20].copy_from_slice(&recs[i].2.to_le_bytes());
        toc[p + 20..p + 24].copy_from_slice(&pk.version.to_le_bytes());
        toc[p + 24] = pk.kind;
        toc[p + 25] = pk.flags;
        let (mo, ms) = if metas[i].is_empty() {
            (0u64, 0u32)
        } else {
            (blob_off as u64 + meta_off[i], metas[i].len() as u32)
        };
        toc[p + 32..p + 40].copy_from_slice(&mo.to_le_bytes());
        toc[p + 40..p + 44].copy_from_slice(&ms.to_le_bytes());
        p += 0x40;
    }
    for d in &t.dirs {
        toc[p..p + 4].copy_from_slice(&d.0.to_le_bytes());
        toc[p + 4..p + 8].copy_from_slice(&d.1.to_le_bytes());
        toc[p + 8..p + 12].copy_from_slice(&d.2.to_le_bytes());
        toc[p + 12..p + 16].copy_from_slice(&d.3.to_le_bytes());
        p += 0x18;
    }
    for f in &t.files {
        toc[p..p + 4].copy_from_slice(&f.0.to_le_bytes());
        toc[p + 4..p + 8].copy_from_slice(&f.1.to_le_bytes());
        toc[p + 8..p + 16].copy_from_slice(&f.2.to_le_bytes());
        let eoff = if f.2 == 0 { 0 } else { blob_off as u64 + f.3 };
        toc[p + 16..p + 24].copy_from_slice(&eoff.to_le_bytes());
        toc[p + 24..p + 32].copy_from_slice(&f.4.to_le_bytes());
        toc[p + 32..p + 64].copy_from_slice(&f.5);
        toc[p + 64] = f.6;
        p += 0x50;
    }
    for s in &t.slots {
        toc[p..p + 4].copy_from_slice(&s.to_le_bytes());
        p += 4;
    }
    toc[p..].copy_from_slice(&t.pool);
    let mut out = vec![0u8; 0x80];
    out[..4].copy_from_slice(b"NUP\x01");
    out[4..6].copy_from_slice(&0x80u16.to_le_bytes());
    out[8..16].copy_from_slice(&0x80u64.to_le_bytes());
    out[16..24].copy_from_slice(&(toc_size as u64).to_le_bytes());
    out[24..56].copy_from_slice(&Sha256::digest(&toc));
    out[56..64].copy_from_slice(&(blob_off as u64).to_le_bytes());
    out[64..72].copy_from_slice(&total_override.unwrap_or(total).to_le_bytes());
    out[72..76].copy_from_slice(&(npkg as u32).to_le_bytes());
    out.extend_from_slice(&toc);
    out.resize(blob_off, 0);
    out.extend_from_slice(&t.blob);
    out
}

// ---- helpers ------------------------------------------------------------------------------------------------------------------------

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp(name: &str) -> PathBuf {
    let base = std::env::var("IBUILDER_TEST_TMP")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let d = base.join(format!(
        "ibuilder-nup-test-{}-{}-{name}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn bytes(seed: u64, n: usize) -> Vec<u8> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 33) as u8
        })
        .collect()
}

fn text(n: usize) -> Vec<u8> {
    b"Hyrule field, a small drum, and a very long ocarina tune. "
        .iter()
        .cycle()
        .take(n)
        .copied()
        .collect()
}

const APP: u64 = 0x0400_4860_24DE_C000;

fn romfs_tree() -> Vec<Node> {
    vec![
        file("zzz.bin", &bytes(1, 5000)),
        dir(
            "Pack",
            vec![
                file("ZsDic.pack.zs", &bytes(2, 70_000)),
                zfile("big.dat", &text(3_000_000)),
                zfile("zero", &[]),
                file(".listing", b"an artifact: left out"),
            ],
        ),
        dir(
            "Event",
            vec![
                dir(
                    "Movie",
                    vec![
                        zfile("a.webm", &bytes(3, 300_000)),
                        file("b.webm", &bytes(4, 130_001)),
                    ],
                ),
                file("t.txt", &text(1000)),
            ],
        ),
        file("aaa", b"first"),
    ]
}

fn update(version: u32, pid: u64, romfs: Option<Vec<Node>>, meta: Vec<(u16, Vec<u8>)>) -> Pkg {
    Pkg {
        title_id: pid | 0x800,
        kind: 1,
        version,
        flags: 0,
        meta,
        exefs: Some(vec![
            file("main", &bytes(10 + version as u64, 4096)),
            file("main.npdm", &npdm(pid)),
            file("rtld", &bytes(11, 100)),
            file("sdk", &text(2000)),
            file(".hidden", b"x"),
            zfile("subsdk0", &text(9000)),
        ]),
        romfs,
    }
}

fn base(pid: u64, romfs: Vec<Node>) -> Pkg {
    Pkg {
        title_id: pid,
        kind: 0,
        version: 65536,
        flags: 0,
        meta: vec![(2, nacp(APP, APP + 0x1000)), (3, b"JPEGDATA".to_vec())],
        exefs: Some(vec![
            file("main", &bytes(9, 3000)),
            file("main.npdm", &npdm(pid)),
        ]),
        romfs: Some(romfs),
    }
}

fn container(dir: &Path, name: &str, pkgs: &[Pkg]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, build(pkgs, None)).unwrap();
    p
}

fn expected_romfs(nodes: &[Node], prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
    for n in nodes {
        match n {
            Node::Dir(d, k) => expected_romfs(k, &format!("{prefix}{d}/"), out),
            Node::File(f, data, _) if f != ".listing" => {
                out.insert(format!("{prefix}{f}"), data.clone());
            }
            _ => {}
        }
    }
}

/// every file of the image against what the tree says: the DMROMFS1 index read back, the bytes at each offset
fn check_image(img: &Path, want: &BTreeMap<String, Vec<u8>>) {
    use std::os::unix::fs::FileExt;
    let index = read_index(img).unwrap();
    let f = std::fs::File::open(img).unwrap();
    let mut seen = 0;
    for (path, is_dir, off, size) in &index {
        if *is_dir {
            continue;
        }
        let want_bytes = want
            .get(path)
            .unwrap_or_else(|| panic!("{path} is in the image, not in the tree"));
        let mut b = vec![0u8; *size as usize];
        f.read_exact_at(&mut b, *off).unwrap();
        assert!(&b == want_bytes, "{path} differs");
        seen += 1;
    }
    assert_eq!(seen, want.len());
}

fn opts_with(threads: usize) -> Options {
    Options {
        threads,
        ..Options::default()
    }
}

// ---- tests -----------------------------------------------------------------------------------------------------------------------------

#[test]
fn a_base_with_an_update_and_a_dlc_is_scanned_and_extracted_like_the_python() {
    let d = tmp("full");
    let dlc = Pkg {
        title_id: APP + 0x1000 + 3,
        kind: 2,
        version: 0,
        flags: 0,
        meta: vec![],
        exefs: None,
        romfs: Some(vec![file("dlc.bin", &bytes(77, 1234))]),
    };
    let nup = container(
        &d,
        "g.nup",
        &[
            base(APP, romfs_tree()),
            update(
                131072,
                APP,
                Some(romfs_tree()),
                vec![(2, nacp(APP, APP + 0x1000)), (3, b"ICON2".to_vec())],
            ),
            dlc,
        ],
    );
    assert!(is_nup(&nup));
    let n = Nup::open(&nup).unwrap();
    let sc = scan(&n, None).unwrap();
    assert_eq!(
        (sc.program_id, sc.application, sc.aoc_base),
        (APP, APP, APP + 0x1000)
    );
    assert_eq!(
        sc.versions.keys().copied().collect::<Vec<_>>(),
        vec![131072]
    );
    assert_eq!(
        sc.dlc.keys().copied().collect::<Vec<_>>(),
        vec![APP + 0x1000 + 3]
    );
    let out = d.join("extract");
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    let mut o = opts_with(3);
    o.say = Some(Arc::new(move |l| s2.lock().unwrap().push(l.to_string())));
    let x = extract_application(
        &nup,
        &out,
        Some(&{
            let a = d.join("aoc");
            move |_: &str| a.clone()
        }),
        None,
        None,
        Some(n),
        &o,
    )
    .unwrap();
    assert_eq!(
        (x.title_id.as_str(), x.version, x.has_base, x.dlc, x.how),
        ("0400486024DEC000", 131072, true, 1, How::Packed)
    );
    // ExeFS: the files that do not start with a dot, byte for byte
    let mut names: Vec<String> = std::fs::read_dir(out.join("ExeFS"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["main", "main.npdm", "rtld", "sdk", "subsdk0"]);
    assert_eq!(
        std::fs::read(out.join("ExeFS/main")).unwrap(),
        bytes(10 + 131072, 4096)
    );
    assert_eq!(
        std::fs::read(out.join("ExeFS/subsdk0")).unwrap(),
        text(9000)
    );
    // control data: the update's own icon, its NACP
    assert_eq!(std::fs::read(out.join("icon.jpg")).unwrap(), b"ICON2");
    assert_eq!(
        std::fs::read(out.join("control.nacp")).unwrap().len(),
        0x4000
    );
    // RomFS: every file, the artifact left out
    let mut want = BTreeMap::new();
    expected_romfs(&romfs_tree(), "", &mut want);
    check_image(&out.join("RomFS.img"), &want);
    // the DLC at its index
    let mut dw = BTreeMap::new();
    dw.insert("dlc.bin".to_string(), bytes(77, 1234));
    check_image(&d.join("aoc/3.img"), &dw);
    let lines = seen.lock().unwrap().join("\n");
    assert!(
        lines.contains("nup: 0400486024DEC000 version 131072")
            && lines.contains("1 DLC")
            && lines.contains("packing"),
        "{lines}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_image_is_the_same_for_any_thread_count_and_for_both_zstd_backends() {
    let d = tmp("threads");
    let nup = container(
        &d,
        "g.nup",
        &[
            base(APP, romfs_tree()),
            update(131072, APP, Some(romfs_tree()), vec![]),
        ],
    );
    let mut digests = vec![];
    for (i, (threads, ruzstd)) in [(1, false), (7, false), (4, true)].into_iter().enumerate() {
        let mut n = Nup::open(&nup).unwrap();
        if ruzstd {
            n = n.with_backend(Arc::new(Ruzstd));
        }
        let out = d.join(format!("e{i}"));
        extract_application(&nup, &out, None, None, None, Some(n), &opts_with(threads)).unwrap();
        digests.push(Sha256::digest(
            std::fs::read(out.join("RomFS.img")).unwrap(),
        ));
    }
    assert!(digests.windows(2).all(|w| w[0] == w[1]));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_repeat_run_keeps_the_image_and_a_foreign_stamp_makes_it_verify_and_a_damaged_image_is_packed_again()
 {
    let d = tmp("repeat");
    let nup = container(
        &d,
        "g.nup",
        &[
            base(APP, romfs_tree()),
            update(131072, APP, Some(romfs_tree()), vec![]),
        ],
    );
    let out = d.join("extract");
    let o = opts_with(2);
    assert_eq!(
        extract_application(&nup, &out, None, None, None, None, &o)
            .unwrap()
            .how,
        How::Packed
    );
    let img = out.join("RomFS.img");
    let before = std::fs::metadata(&img).unwrap().modified().unwrap();
    assert_eq!(
        extract_application(&nup, &out, None, None, None, None, &o)
            .unwrap()
            .how,
        How::Kept
    );
    assert_eq!(std::fs::metadata(&img).unwrap().modified().unwrap(), before);
    // the Python's stamp (another source digest): checked against the container's hashes, then stamped
    std::fs::write(
        out.join("RomFS.img.inputs.json"),
        r#"{"source": "from the python", "image": [1, 2, 3, 4]}"#,
    )
    .unwrap();
    assert_eq!(
        extract_application(&nup, &out, None, None, None, None, &o)
            .unwrap()
            .how,
        How::Verified
    );
    assert_eq!(
        extract_application(&nup, &out, None, None, None, None, &o)
            .unwrap()
            .how,
        How::Kept
    );
    // one byte of a file changed: no stamp matches, the check fails, the image is made again
    {
        use std::os::unix::fs::FileExt;
        let f = std::fs::OpenOptions::new().write(true).open(&img).unwrap();
        f.write_all_at(&[0xAA, 0xBB], 40).unwrap();
    }
    assert_eq!(
        extract_application(&nup, &out, None, None, None, None, &o)
            .unwrap()
            .how,
        How::Packed
    );
    let mut want = BTreeMap::new();
    expected_romfs(&romfs_tree(), "", &mut want);
    check_image(&img, &want);
    // an ExeFS that is already right is not written again
    let t = std::fs::metadata(out.join("ExeFS/main"))
        .unwrap()
        .modified()
        .unwrap();
    extract_application(&nup, &out, None, None, None, None, &o).unwrap();
    assert_eq!(
        std::fs::metadata(out.join("ExeFS/main"))
            .unwrap()
            .modified()
            .unwrap(),
        t
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn verify_container_checks_every_file_and_a_damaged_one_is_named() {
    let d = tmp("verify");
    let nup = container(
        &d,
        "g.nup",
        &[
            base(APP, romfs_tree()),
            update(131072, APP, Some(romfs_tree()), vec![]),
        ],
    );
    let n = Nup::open(&nup).unwrap();
    let (files, bytes_) = verify_container(&n, &opts_with(4)).unwrap();
    assert!(
        files >= 14 && bytes_ > 3_000_000,
        "{files} files {bytes_} bytes"
    );
    // flip a byte inside the zstd frame of big.dat, and one inside a raw file
    let (name, idx) = (0..n.file_count())
        .map(|i| (String::from_utf8_lossy(n.file_name(i)).into_owned(), i))
        .find(|(nm, _)| nm == "big.dat")
        .unwrap();
    let (eoff, esz, codec) = n.file_extent(idx);
    assert_eq!(codec, 1);
    drop(n);
    let mut data = std::fs::read(&nup).unwrap();
    data[(eoff + esz / 2) as usize] ^= 0x55;
    std::fs::write(&nup, &data).unwrap();
    let n = Nup::open(&nup).unwrap();
    let e = verify_container(&n, &opts_with(4)).unwrap_err().to_string();
    assert!(
        e.contains(&name) && (e.contains("SHA-256") || e.contains("zstd")),
        "{e}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_container_that_is_not_one_is_refused_with_the_pythons_messages() {
    let d = tmp("bad");
    let good = build(&[base(APP, romfs_tree())], None);
    let open = |b: Vec<u8>| -> String {
        let p = d.join("x.nup");
        std::fs::write(&p, b).unwrap();
        Nup::open(&p)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "OK".into())
    };
    assert!(open(vec![]).contains("empty file"));
    assert!(open(b"NUP".to_vec()).contains("too short"));
    let mut b = good.clone();
    b[0] = b'X';
    assert!(open(b).contains("not a NUP"));
    let mut b = good.clone();
    b[3] = 2;
    assert!(open(b).contains("format version 2"));
    assert!(
        open(good[..good.len() - 100].to_vec()).contains("its header says"),
        "a cut container"
    );
    let mut b = good.clone();
    b[0x80 + 10] ^= 1; // a TOC byte
    assert!(open(b).contains("does not match its SHA-256"));
    let mut b = good.clone();
    b[76] = 1; // flags
    assert!(open(b).contains("unsupported NUP header"));
    assert_eq!(open(good.clone()), "OK");
    // a folder that contains itself: the slots of folder 0 name folder 0
    let n = Nup::from_source(Box::new(MemSource(good.clone())), Path::new("g.nup")).unwrap();
    assert!(n.dir_count() > 3);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_no_base_container_takes_its_standalone_update_as_the_game_and_a_two_program_one_packs_both() {
    let d = tmp("nobase");
    // no base record, an update marked standalone: the game, with its control data from the update itself
    let mut up = update(
        65537,
        APP,
        Some(romfs_tree()),
        vec![(2, nacp(APP, 0)), (3, b"I".to_vec())],
    );
    up.flags = 1;
    let nup = container(&d, "u.nup", &[up]);
    let out = d.join("e1");
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    let mut o = opts_with(2);
    o.say = Some(Arc::new(move |l| s2.lock().unwrap().push(l.to_string())));
    let x = extract_application(&nup, &out, None, None, None, None, &o).unwrap();
    assert_eq!((x.version, x.has_base), (65537, false));
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|l| l.starts_with("no base game in this NUP"))
    );
    assert_eq!(std::fs::read(out.join("icon.jpg")).unwrap(), b"I");
    // an update that is not marked standalone with no base: refused
    let nup2 = container(
        &d,
        "v.nup",
        &[update(65537, APP, Some(romfs_tree()), vec![])],
    );
    let e = extract_application(&nup2, &d.join("e2"), None, None, None, None, &o)
        .unwrap_err()
        .to_string();
    assert!(e.contains("no base game: pack the base game"), "{e}");
    // a Switch 2 Edition: programs 0 and 1 of one application; the highest is packed, the other's RomFS goes beside it
    let p0 = APP;
    let p1 = APP | 1;
    let a = Pkg {
        title_id: p0,
        kind: 0,
        version: 65536,
        flags: 0,
        meta: vec![(2, nacp(APP, 0))],
        exefs: Some(vec![file("main", b"m0"), file("main.npdm", &npdm(p0))]),
        romfs: Some(vec![file("one.bin", b"program zero")]),
    };
    let b = Pkg {
        title_id: p1,
        kind: 0,
        version: 65536,
        flags: 0,
        meta: vec![(2, nacp(APP, 0))],
        exefs: Some(vec![file("main", b"m1"), file("main.npdm", &npdm(p1))]),
        romfs: Some(vec![file("two.bin", b"program one")]),
    };
    let nup3 = container(&d, "w.nup", &[a, b]);
    let out3 = d.join("e3");
    let x = extract_application(&nup3, &out3, None, None, None, None, &o).unwrap();
    assert_eq!(x.program_id, p1);
    assert_eq!(std::fs::read(out3.join("ExeFS/main")).unwrap(), b"m1");
    let mut w = BTreeMap::new();
    w.insert("two.bin".to_string(), b"program one".to_vec());
    check_image(&out3.join("RomFS.img"), &w);
    let mut w = BTreeMap::new();
    w.insert("one.bin".to_string(), b"program zero".to_vec());
    check_image(&out3.join("romfs_programs/0.img"), &w);
    // --program 0 packs the other one
    let out4 = d.join("e4");
    assert_eq!(
        extract_application(&nup3, &out4, None, Some(0), None, None, &o)
            .unwrap()
            .program_id,
        p0
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn cancellation_stops_the_extraction_and_leaves_no_image() {
    let d = tmp("cancel");
    let nup = container(
        &d,
        "g.nup",
        &[
            base(APP, romfs_tree()),
            update(131072, APP, Some(romfs_tree()), vec![]),
        ],
    );
    // cancelled before it starts
    let out = d.join("e0");
    let o = opts_with(2);
    o.cancel.store(true, Ordering::Relaxed);
    assert!(matches!(
        extract_application(&nup, &out, None, None, None, None, &o),
        Err(Error::Cancelled)
    ));
    assert!(!out.join("RomFS.img").exists());
    // cancelled by the progress callback after some bytes
    let out = d.join("e1");
    let mut o = opts_with(3);
    o.progress_ms = 0;
    let flag = o.cancel.clone();
    let hits = Arc::new(AtomicU64::new(0));
    let h = hits.clone();
    o.progress = Some(Arc::new(move |p: &Progress| {
        if p.step == "unpacking" {
            h.fetch_add(1, Ordering::Relaxed);
            if p.done_bytes > 100_000 {
                flag.store(true, Ordering::Relaxed);
            }
        }
    }));
    let r = extract_application(&nup, &out, None, None, None, None, &o);
    assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
    assert!(
        !out.join("RomFS.img").exists() && !out.join("RomFS.img.part").exists(),
        "no image, no part file"
    );
    assert!(hits.load(Ordering::Relaxed) >= 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_free_space_check_refuses_and_a_present_image_is_not_asked_again() {
    let d = tmp("space");
    let nup = container(
        &d,
        "g.nup",
        &[
            base(APP, romfs_tree()),
            update(131072, APP, Some(romfs_tree()), vec![]),
        ],
    );
    let out = d.join("extract");
    let mut o = opts_with(2);
    o.free_bytes = Some(Arc::new(|_| Some(10 << 20)));
    let e = extract_application(&nup, &out, None, None, None, None, &o)
        .unwrap_err()
        .to_string();
    assert!(
        e.starts_with("not enough space for g.nup: about") && e.contains("GB needed"),
        "{e}"
    );
    assert!(!out.exists() || !out.join("RomFS.img").exists());
    // with room, then with none: the image that is there at the right size is kept (the Python would refuse a nearly full volume)
    let ok = opts_with(2);
    extract_application(&nup, &out, None, None, None, None, &ok).unwrap();
    assert_eq!(
        extract_application(&nup, &out, None, None, None, None, &o)
            .unwrap()
            .how,
        How::Kept
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_zstd_file_with_trailing_bytes_or_a_wrong_size_is_damaged() {
    let d = tmp("zstd");
    let data = text(100_000);
    let nup = container(
        &d,
        "g.nup",
        &[
            base(APP, vec![zfile("a.zs", &data)]),
            update(131072, APP, Some(vec![zfile("a.zs", &data)]), vec![]),
        ],
    );
    let mut raw = std::fs::read(&nup).unwrap();
    let n = Nup::open(&nup).unwrap();
    let i = (0..n.file_count())
        .find(|&i| n.file_name(i) == b"a.zs")
        .unwrap();
    let (eoff, esz, _) = n.file_extent(i);
    drop(n);
    // the frame's last byte (its checksum or the last block) damaged
    raw[(eoff + esz - 1) as usize] ^= 1;
    std::fs::write(&nup, &raw).unwrap();
    let n = Nup::open(&nup).unwrap();
    assert!(n.read(i).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn both_zstd_backends_decode_the_same_frames() {
    let d = tmp("backends");
    let big = text(5_000_000);
    let nup = container(
        &d,
        "g.nup",
        &[base(
            APP,
            vec![
                zfile("a", &big),
                zfile("b", &bytes(5, 1_000_000)),
                zfile("c", b"tiny"),
            ],
        )],
    );
    let cancel = AtomicBool::new(false);
    for backend in [default_backend(), Arc::new(Ruzstd)] {
        let n = Nup::open(&nup).unwrap().with_backend(backend);
        for i in 0..n.file_count() {
            let (_, _, codec) = n.file_extent(i);
            if codec == 1 {
                let mut total = 0u64;
                let got = n
                    .copy(
                        i,
                        &mut |c| {
                            total += c.len() as u64;
                            Ok(())
                        },
                        &cancel,
                    )
                    .unwrap();
                assert_eq!((got, total), (n.file_size(i), n.file_size(i)));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&d);
}

// ---- the cover of a .nup before any build (`application_icon`: the head of the file only) ----------------------------------------------------------------------

type MetaList = Vec<(u16, Vec<u8>)>;
type Edit = Box<dyn Fn(&mut [u8])>;

/// the localized icons record (TLV 10): `[tag length u8][tag][data length u32 LE][data]` for each language
fn licons(set: &[(&str, &[u8])]) -> Vec<u8> {
    let mut v = vec![];
    for (tag, data) in set {
        v.push(tag.len() as u8);
        v.extend_from_slice(tag.as_bytes());
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
    }
    v
}

/// a base package with the given meta and a small tree
fn base_with(pid: u64, meta: MetaList, romfs: Vec<Node>) -> Pkg {
    Pkg {
        title_id: pid,
        kind: 0,
        version: 65536,
        flags: 0,
        meta,
        exefs: Some(vec![
            file("main", &bytes(9, 3000)),
            file("main.npdm", &npdm(pid)),
        ]),
        romfs: Some(romfs),
    }
}

fn small_tree() -> Vec<Node> {
    vec![
        file("a.bin", &bytes(21, 900)),
        dir("d", vec![file("b.bin", &text(700))]),
    ]
}

/// what the BUILD writes to icon.jpg for this container (None: no icon.jpg), and the title it names the game by
fn build_icon(d: &Path, nup: &Path, tag: &str) -> (Option<Vec<u8>>, String) {
    let out = d.join(format!("build-{tag}"));
    let x = extract_application(nup, &out, None, None, None, None, &opts_with(2)).unwrap();
    (std::fs::read(out.join("icon.jpg")).ok(), x.title_id)
}

#[test]
fn the_cover_is_the_icon_the_build_writes_and_the_title_the_build_names() {
    let d = tmp("cover");
    let en: &[u8] = b"ENGLISH-JPEG";
    let fr: &[u8] = b"FRENCH-JPEG";
    let de: &[u8] = b"GERMAN-JPEG";
    let cases: Vec<(&str, MetaList, Option<&[u8]>)> = vec![
        // the icon record wins over the localized ones
        (
            "plain",
            vec![
                (2, nacp(APP, APP + 0x1000)),
                (3, b"JPEGDATA".to_vec()),
                (10, licons(&[("en-US", en)])),
            ],
            Some(b"JPEGDATA"),
        ),
        // localized only: en-US wherever it is in the list
        (
            "en-us",
            vec![(10, licons(&[("fr", fr), ("en-US", en), ("de", de)]))],
            Some(en),
        ),
        // the primary subtag of en-US, when the exact tag is not there
        (
            "en",
            vec![(10, licons(&[("fr", fr), ("en", en)]))],
            Some(en),
        ),
        // no English at all: the first localized one
        (
            "first",
            vec![(10, licons(&[("fr", fr), ("de", de)]))],
            Some(fr),
        ),
        // the first one is empty: none (the build's rule: it takes the first and drops it when it is empty)
        (
            "empty-first",
            vec![(10, licons(&[("fr", b""), ("de", de)]))],
            None,
        ),
        // an empty icon record is no icon, the localized one is used
        (
            "empty-plain",
            vec![(3, vec![]), (10, licons(&[("de", de)]))],
            Some(de),
        ),
        // none at all (the real Ocarina container: NPDM and three small records, no NACP, no icon)
        (
            "none",
            vec![
                (1, npdm(APP)),
                (5, vec![0; 36]),
                (7, vec![0; 32]),
                (8, vec![0; 4]),
            ],
            None,
        ),
        ("nacp-only", vec![(2, nacp(APP, APP + 0x1000))], None),
        ("no-meta", vec![], None),
    ];
    for (tag, meta, want) in cases {
        let nup = container(
            &d,
            &format!("{tag}.nup"),
            &[base_with(APP, meta, small_tree())],
        );
        let cover = application_icon(&nup).unwrap_or_else(|e| panic!("{tag}: {e}"));
        let (built, title) = build_icon(&d, &nup, tag);
        assert_eq!(cover.icon.as_deref(), want, "{tag}");
        assert_eq!(
            cover.icon, built,
            "{tag}: the cover is not what the build writes to icon.jpg"
        );
        assert_eq!(
            cover.title_id.map(|t| format!("{t:016X}")),
            Some(title),
            "{tag}: the title is not the build's"
        );
        assert!(
            cover.package.ends_with("(base)"),
            "{tag}: {}",
            cover.package
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_base_with_no_icon_takes_the_icon_the_build_writes_from_its_update() {
    let d = tmp("cover-update");
    let none: MetaList = vec![
        (1, npdm(APP)),
        (5, vec![0; 36]),
        (7, vec![0; 32]),
        (8, vec![0; 4]),
    ];
    let upd_icon: MetaList = vec![(2, nacp(APP, APP + 0x1000)), (3, b"UPDATE-JPEG".to_vec())];
    // the base carries none (as the real Ocarina container's), its update does: the build packs the update and writes its icon; the cover is that icon
    let nup = container(
        &d,
        "upd.nup",
        &[
            base_with(APP, none.clone(), small_tree()),
            update(131072, APP, None, upd_icon.clone()),
        ],
    );
    let cover = application_icon(&nup).unwrap();
    let (built, title) = build_icon(&d, &nup, "upd");
    assert_eq!(cover.icon.as_deref(), Some(&b"UPDATE-JPEG"[..]));
    assert_eq!(
        cover.icon, built,
        "the cover is not what the build writes to icon.jpg"
    );
    assert_eq!(cover.title_id.map(|t| format!("{t:016X}")), Some(title));
    assert!(!cover.package.ends_with("(base)"), "{}", cover.package);
    // neither carries one: none, as the build
    let nup = container(
        &d,
        "none2.nup",
        &[
            base_with(APP, none.clone(), small_tree()),
            update(131072, APP, None, vec![(5, vec![0; 36])]),
        ],
    );
    let cover = application_icon(&nup).unwrap();
    let (built, _) = build_icon(&d, &nup, "none2");
    assert_eq!((cover.icon.clone(), built), (None, None));
    // the base carries one and the update none: the base's stays the cover (as before; the build of the update writes none)
    let nup = container(
        &d,
        "baseicon.nup",
        &[
            base_with(APP, vec![(3, b"BASE-JPEG".to_vec())], small_tree()),
            update(131072, APP, None, vec![(5, vec![0; 36])]),
        ],
    );
    let cover = application_icon(&nup).unwrap();
    assert_eq!(cover.icon.as_deref(), Some(&b"BASE-JPEG"[..]));
    assert!(cover.package.ends_with("(base)"), "{}", cover.package);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_two_program_container_is_named_by_the_program_the_build_packs_and_a_missing_npdm_falls_back_to_the_meta_record()
 {
    let d = tmp("cover-programs");
    let (p0, p1) = (APP, APP | 1);
    let mk = |pid: u64, meta: MetaList| Pkg {
        title_id: pid,
        kind: 0,
        version: 65536,
        flags: 0,
        meta,
        exefs: Some(vec![file("main", b"m"), file("main.npdm", &npdm(pid))]),
        romfs: Some(vec![file("r", b"r")]),
    };
    // a Switch 2 Edition: the icon is the first base package's (`app_package`), the title is the build's (the highest program)
    let nup = container(
        &d,
        "two.nup",
        &[
            mk(p0, vec![(2, nacp(APP, 0)), (3, b"ICON0".to_vec())]),
            mk(p1, vec![(2, nacp(APP, 0)), (3, b"ICON1".to_vec())]),
        ],
    );
    let cover = application_icon(&nup).unwrap();
    let (_, title) = build_icon(&d, &nup, "two");
    assert_eq!(
        (cover.icon.as_deref(), cover.title_id),
        (Some(&b"ICON0"[..]), Some(p1))
    );
    assert_eq!(format!("{p1:016X}"), title);
    // `scan` refuses a container with no exefs/main.npdm anywhere: the NPDM record of the package's meta (TLV 1) still names the title
    let bare = Pkg {
        title_id: APP,
        kind: 0,
        version: 65536,
        flags: 0,
        meta: vec![(1, npdm(APP)), (3, b"ICON".to_vec())],
        exefs: None,
        romfs: Some(small_tree()),
    };
    let nup = container(&d, "bare.nup", &[bare]);
    assert!(scan(&Nup::open(&nup).unwrap(), None).is_err());
    let cover = application_icon(&nup).unwrap();
    assert_eq!(
        (cover.icon.as_deref(), cover.title_id),
        (Some(&b"ICON"[..]), Some(APP))
    );
    // nothing names it: the icon is still returned, the title is None
    let bare = Pkg {
        title_id: APP,
        kind: 0,
        version: 65536,
        flags: 0,
        meta: vec![(3, b"ICON".to_vec())],
        exefs: None,
        romfs: Some(small_tree()),
    };
    let nup = container(&d, "anon.nup", &[bare]);
    let cover = application_icon(&nup).unwrap();
    assert_eq!(
        (cover.icon.as_deref(), cover.title_id),
        (Some(&b"ICON"[..]), None)
    );
    // an NPDM record that is not an NPDM names nothing
    let bare = Pkg {
        title_id: APP,
        kind: 0,
        version: 65536,
        flags: 0,
        meta: vec![(1, b"not an npdm".to_vec()), (3, b"ICON".to_vec())],
        exefs: None,
        romfs: Some(small_tree()),
    };
    let cover = application_icon(&container(&d, "junk.nup", &[bare])).unwrap();
    assert_eq!(
        (cover.icon.as_deref(), cover.title_id),
        (Some(&b"ICON"[..]), None)
    );
    // a base + update container: the application's package is the base (the build's icon.jpg is the update's own, when it has one)
    let nup = container(
        &d,
        "upd.nup",
        &[
            base(APP, small_tree()),
            update(131072, APP, None, vec![(3, b"ICON2".to_vec())]),
        ],
    );
    assert_eq!(
        application_icon(&nup).unwrap().icon.as_deref(),
        Some(&b"JPEGDATA"[..])
    );
    assert_eq!(
        build_icon(&d, &nup, "upd").0.as_deref(),
        Some(&b"ICON2"[..])
    );
    // a no-base container: its standalone update is the application's package, as for the build
    let mut up = update(65537, APP, Some(small_tree()), vec![(3, b"I".to_vec())]);
    up.flags = 1;
    let nup = container(&d, "alone.nup", &[up]);
    let cover = application_icon(&nup).unwrap();
    assert_eq!(
        (cover.icon, cover.title_id),
        (build_icon(&d, &nup, "alone").0, Some(APP))
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// a reader that counts what is read and refuses to read inside `forbidden` (the game's content)
struct Counting {
    inner: MemSource,
    bytes: Arc<AtomicU64>,
    calls: Arc<AtomicU64>,
    forbidden: Vec<(u64, u64)>,
}

impl ReadAt for Counting {
    fn len(&self) -> u64 {
        self.inner.len()
    }
    fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
        let end = offset + buf.len() as u64;
        if let Some((a, b)) = self.forbidden.iter().find(|(a, b)| offset < *b && end > *a) {
            panic!("the cover read {offset}..{end}, inside the game's content {a}..{b}");
        }
        let n = self.inner.read_at(buf, offset)?;
        self.bytes.fetch_add(n as u64, Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(n)
    }
}

/// (bytes read, calls, size of the container, size of its table of contents) of the cover of `image`, with every extent of a file over `big` bytes forbidden
fn counted(image: Vec<u8>, name: &str, big: u64) -> (AppIcon, u64, u64, u64, u64) {
    let toc = u64::from_le_bytes(image[16..24].try_into().unwrap());
    let probe = Nup::from_source(Box::new(MemSource(image.clone())), Path::new(name)).unwrap();
    let forbidden: Vec<(u64, u64)> = (0..probe.file_count())
        .filter(|&i| probe.file_size(i) >= big)
        .map(|i| probe.file_extent(i))
        .map(|(o, s, _)| (o, o + s))
        .collect();
    assert!(
        !forbidden.is_empty(),
        "no file of {big} bytes or more in the container"
    );
    let (bytes_read, calls) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let len = image.len() as u64;
    let src = Counting {
        inner: MemSource(image),
        bytes: bytes_read.clone(),
        calls: calls.clone(),
        forbidden,
    };
    let cover = application_icon_from(Box::new(src), Path::new(name)).unwrap();
    (
        cover,
        bytes_read.load(Ordering::Relaxed),
        calls.load(Ordering::Relaxed),
        len,
        toc,
    )
}

#[test]
fn the_cover_reads_the_head_of_a_big_container_and_none_of_its_content() {
    // a game file of 8 MB, then of 64 MB, a zstd file of 4 MB, a RomFS tree of 3,000 small files, an icon and an NPDM: the same head, ten times the content
    let icon = bytes(77, 150_000);
    let container_with = |big: usize| {
        let mut tree = vec![
            file("big.bin", &bytes(5, big)),
            zfile("big.zs", &text(4 << 20)),
        ];
        for g in 0..30 {
            tree.push(dir(
                &format!("d{g}"),
                (0..100)
                    .map(|i| {
                        file(
                            &format!("f{i}.dat"),
                            &bytes((g * 100 + i) as u64, 64 + i as usize),
                        )
                    })
                    .collect(),
            ));
        }
        build(
            &[base_with(
                APP,
                vec![(2, nacp(APP, APP + 0x1000)), (3, icon.clone())],
                tree,
            )],
            None,
        )
    };
    let mut reads = vec![];
    for (name, big) in [("small.nup", 8usize << 20), ("big.nup", 64 << 20)] {
        let (cover, read, calls, total, toc) = counted(container_with(big), name, 1 << 20);
        eprintln!(
            "{name}: {total} bytes, read {read} bytes in {calls} reads ({:.4}% of it; the table of contents is {toc} bytes, the icon {} bytes)",
            read as f64 * 100.0 / total as f64,
            icon.len()
        );
        assert_eq!(cover.icon.as_deref(), Some(&icon[..]));
        assert_eq!(cover.title_id, Some(APP));
        // the head (superblock + table of contents), the meta block (NACP + icon), the NPDM: nothing proportional to the game
        // (`scan` asks for each package's NACP again, which reads the meta block again: three times the meta block at most)
        let meta_len = 0x4000 + icon.len() as u64 + 16;
        assert!(
            read <= 0x80 + toc + 3 * meta_len + 16 * 1024,
            "read {read} bytes of {total}"
        );
        assert!(calls <= 16, "{calls} reads");
        reads.push((read, calls));
    }
    // eight times the game, the very same reads
    assert_eq!(reads[0], reads[1]);
}

#[test]
fn a_header_that_claims_a_huge_meta_block_is_refused_before_it_becomes_an_allocation() {
    // a container that says it is 8 GiB long, whose meta block claims 1.8 GiB: refused by its size alone (no read of it, no allocation)
    struct Inflated(MemSource, u64);
    impl ReadAt for Inflated {
        fn len(&self) -> u64 {
            self.1
        }
        fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
            let n = if offset >= self.0.len() {
                0
            } else {
                self.0.read_at(buf, offset)?
            };
            buf[n..].fill(0);
            Ok(buf.len())
        }
    }
    let image = build(
        &[base_with(APP, vec![(3, b"I".to_vec())], small_tree())],
        None,
    );
    let mut image = resign(image, |toc| {
        toc[0x30 + 40..0x30 + 44].copy_from_slice(&0x7000_0000u32.to_le_bytes())
    });
    let eight_gib: u64 = 8 << 30;
    image[64..72].copy_from_slice(&eight_gib.to_le_bytes());
    let blob = u64::from_le_bytes(image[56..64].try_into().unwrap());
    assert!(blob < eight_gib);
    let e = application_icon_from(
        Box::new(Inflated(MemSource(image), eight_gib)),
        Path::new("huge.nup"),
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("implausible"), "{e}");
}

/// `image` with its table of contents edited by `edit` and the superblock's SHA-256 of it made again (a damaged INDEX that passes the checksum)
fn resign(mut image: Vec<u8>, edit: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let toc_off = u64::from_le_bytes(image[8..16].try_into().unwrap()) as usize;
    let toc_size = u64::from_le_bytes(image[16..24].try_into().unwrap()) as usize;
    edit(&mut image[toc_off..toc_off + toc_size]);
    let sha: [u8; 32] = Sha256::digest(&image[toc_off..toc_off + toc_size]).into();
    image[24..56].copy_from_slice(&sha);
    image
}

fn cover_of(image: Vec<u8>) -> Result<AppIcon> {
    application_icon_from(Box::new(MemSource(image)), Path::new("damaged.nup"))
}

#[test]
fn a_truncated_head_is_an_error_at_every_length_never_a_panic() {
    let image = build(
        &[
            base_with(
                APP,
                vec![(2, nacp(APP, 0)), (3, b"ICON".to_vec())],
                small_tree(),
            ),
            update(131072, APP, None, vec![(3, b"ICON2".to_vec())]),
        ],
        None,
    );
    assert!(cover_of(image.clone()).is_ok());
    for len in 0..image.len() {
        // cut as a copy or a download cuts it: the header still says the whole length
        let cut = image[..len].to_vec();
        assert!(
            cover_of(cut.clone()).is_err(),
            "a container cut at {len} of {} gave a cover",
            image.len()
        );
        // cut, and the header made to agree with the cut (what a hostile or buggy writer would leave)
        if len >= 0x80 {
            let mut agree = cut;
            agree[64..72].copy_from_slice(&(len as u64).to_le_bytes());
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cover_of(agree)));
            assert!(r.is_ok(), "a panic at length {len}");
        }
    }
}

#[test]
fn a_damaged_index_that_passes_its_checksum_is_an_error_never_a_panic() {
    let image = build(
        &[base_with(
            APP,
            vec![(2, nacp(APP, 0)), (3, b"ICON".to_vec())],
            small_tree(),
        )],
        None,
    );
    assert!(cover_of(image.clone()).is_ok());
    // the TOC: 0x30 header, then the 0x40-byte package record (kind at +24, flags +25, meta offset +32, meta size +40, root +8, exefs +12, romfs +16), the folder records (0x18), the file records (0x50)
    let pkg = 0x30;
    let edits: Vec<(&str, Edit)> = vec![
        ("kind", Box::new(move |t: &mut [u8]| t[pkg + 24] = 9)),
        (
            "reserved flags",
            Box::new(move |t: &mut [u8]| t[pkg + 25] = 0x80),
        ),
        (
            "meta past the end",
            Box::new(move |t: &mut [u8]| {
                t[pkg + 32..pkg + 40].copy_from_slice(&u64::MAX.to_le_bytes())
            }),
        ),
        (
            "meta size past the end",
            Box::new(move |t: &mut [u8]| {
                t[pkg + 40..pkg + 44].copy_from_slice(&u32::MAX.to_le_bytes())
            }),
        ),
        (
            "root folder out of range",
            Box::new(move |t: &mut [u8]| {
                t[pkg + 8..pkg + 12].copy_from_slice(&5000u32.to_le_bytes())
            }),
        ),
        (
            "exefs folder out of range",
            Box::new(move |t: &mut [u8]| {
                t[pkg + 12..pkg + 16].copy_from_slice(&0x7fff_ffffu32.to_le_bytes())
            }),
        ),
        (
            "package count 0",
            Box::new(|t: &mut [u8]| t[4..8].copy_from_slice(&0u32.to_le_bytes())),
        ),
        (
            "package count too high",
            Box::new(|t: &mut [u8]| t[4..8].copy_from_slice(&0x1000_0000u32.to_le_bytes())),
        ),
        (
            "folder count too high",
            Box::new(|t: &mut [u8]| t[8..16].copy_from_slice(&u64::MAX.to_le_bytes())),
        ),
        (
            "file count too high",
            Box::new(|t: &mut [u8]| t[16..24].copy_from_slice(&(1u64 << 40).to_le_bytes())),
        ),
        (
            "name pool offset",
            Box::new(|t: &mut [u8]| t[24..32].copy_from_slice(&u64::MAX.to_le_bytes())),
        ),
        (
            "name pool size",
            Box::new(|t: &mut [u8]| t[32..40].copy_from_slice(&u64::MAX.to_le_bytes())),
        ),
        (
            "first folder's name out of the pool",
            Box::new(move |t: &mut [u8]| {
                t[0x30 + 0x40..0x30 + 0x40 + 4].copy_from_slice(&0x7fff_fff0u32.to_le_bytes())
            }),
        ),
        (
            "first folder's children out of the slots",
            Box::new(move |t: &mut [u8]| {
                t[0x30 + 0x40 + 8..0x30 + 0x40 + 12].copy_from_slice(&0xffff_fff0u32.to_le_bytes())
            }),
        ),
        (
            "first folder's child count",
            Box::new(move |t: &mut [u8]| {
                t[0x30 + 0x40 + 12..0x30 + 0x40 + 16].copy_from_slice(&u32::MAX.to_le_bytes())
            }),
        ),
    ];
    for (what, edit) in edits {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cover_of(resign(image.clone(), |t| edit(t)))
        }));
        match r {
            Ok(Err(_)) => {}
            Ok(Ok(c)) => panic!("{what}: a cover was given for a damaged index: {c:?}"),
            Err(_) => panic!("{what}: panic"),
        }
    }
    // a flipped byte of the TOC without the checksum made again: the checksum says so
    let mut flipped = image.clone();
    flipped[0x80 + 0x31] ^= 0xff;
    assert!(
        cover_of(flipped)
            .unwrap_err()
            .to_string()
            .contains("SHA-256")
    );
}

#[test]
fn single_byte_corruptions_of_the_whole_head_never_panic() {
    let image = build(
        &[
            base_with(
                APP,
                vec![
                    (2, nacp(APP, 0)),
                    (3, b"ICON".to_vec()),
                    (10, licons(&[("fr", b"F"), ("en-US", b"E")])),
                ],
                small_tree(),
            ),
            update(131072, APP, None, vec![(3, b"ICON2".to_vec())]),
        ],
        None,
    );
    let toc_end = 0x80 + u64::from_le_bytes(image[16..24].try_into().unwrap()) as usize;
    let mut s: u64 = 0x1234_5678;
    let mut errors = 0;
    for round in 0..4000 {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let at = (s >> 33) as usize % toc_end;
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let flip = ((s >> 33) as u8) | 1;
        let mut bad = image.clone();
        bad[at] ^= flip;
        // inside the TOC the checksum is made again, so the damage reaches the parser; in the superblock it is left alone
        let bad = if at >= 0x80 { resign(bad, |_| {}) } else { bad };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cover_of(bad))) {
            Ok(Ok(_)) => {}
            Ok(Err(_)) => errors += 1,
            Err(_) => panic!("round {round}: a panic with byte {at} xor {flip:#x}"),
        }
    }
    assert!(
        errors > 500,
        "only {errors} of 4000 corruptions were refused"
    );
}

/// A container with one base package whose root is folder 0, `slots[d]` the child slots of folder `d` (folders only, no file, no data):
/// written by hand so the TOC can say what the tree writer above never writes.
fn folders_only(slots: &[Vec<u32>]) -> Vec<u8> {
    let nd = slots.len();
    let nslots: usize = slots.iter().map(|s| s.len()).sum();
    let pool = b"d";
    let need = 0x30 + 0x40 + nd * 0x18;
    let pool_off = need + 4 * nslots;
    let toc_size = pool_off + pool.len();
    let mut toc = vec![0u8; toc_size];
    toc[..4].copy_from_slice(b"TOC\x01");
    toc[4..8].copy_from_slice(&1u32.to_le_bytes());
    toc[8..16].copy_from_slice(&(nd as u64).to_le_bytes());
    toc[24..32].copy_from_slice(&(pool_off as u64).to_le_bytes());
    toc[32..40].copy_from_slice(&(pool.len() as u64).to_le_bytes());
    let pk = 0x30;
    toc[pk..pk + 8].copy_from_slice(&APP.to_le_bytes());
    toc[pk + 12..pk + 16].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // no ExeFS
    toc[pk + 16..pk + 20].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // no RomFS
    let (mut p, mut first) = (0x70, 0u32);
    for s in slots {
        toc[p + 4..p + 8].copy_from_slice(&1u32.to_le_bytes()); // name "d" at 0
        toc[p + 8..p + 12].copy_from_slice(&first.to_le_bytes());
        toc[p + 12..p + 16].copy_from_slice(&(s.len() as u32).to_le_bytes());
        first += s.len() as u32;
        p += 0x18;
    }
    for &c in slots.iter().flatten() {
        toc[p..p + 4].copy_from_slice(&c.to_le_bytes());
        p += 4;
    }
    toc[p..].copy_from_slice(pool);
    let blob = (0x80 + toc_size + 15) & !15;
    let mut out = vec![0u8; 0x80];
    out[..4].copy_from_slice(b"NUP\x01");
    out[4..6].copy_from_slice(&0x80u16.to_le_bytes());
    out[8..16].copy_from_slice(&0x80u64.to_le_bytes());
    out[16..24].copy_from_slice(&(toc_size as u64).to_le_bytes());
    out[24..56].copy_from_slice(&Sha256::digest(&toc));
    out[56..64].copy_from_slice(&(blob as u64).to_le_bytes());
    out[64..72].copy_from_slice(&(blob as u64).to_le_bytes());
    out[72..76].copy_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&toc);
    out.resize(blob, 0);
    out
}

/// Review 2026-11-20 §4.1: a folder listed by two slots is not a cycle, so the cycle check let it through, and every walk of the tree visits it once
/// per path. 40 folders, each listing the next one twice, is a TOC of ~1.3 KB whose `tree_bytes` (what the cover reads, through `scan`) or
/// `romfs::entries` would visit 2^40 folders. It is refused when the container is opened, like the `DMROMFS1` index's "listed by more than one
/// directory"; the same chain with one slot per folder opens and walks.
#[test]
fn a_folder_listed_by_two_slots_is_refused_before_any_walk_could_go_exponential() {
    const K: u32 = 40;
    let chain = |dup: bool| -> Vec<Vec<u32>> {
        (0..K)
            .map(|i| match (i + 1 < K, dup) {
                (false, _) => vec![],
                (true, false) => vec![i + 1],
                (true, true) => vec![i + 1, i + 1],
            })
            .collect()
    };
    let open = |b: Vec<u8>| Nup::from_source(Box::new(MemSource(b)), Path::new("dag.nup"));
    // the control: a plain chain is a tree, it opens and its walk is linear
    let n = open(folders_only(&chain(false))).expect("a chain of folders is a valid tree");
    assert_eq!(n.dir_count(), K as usize);
    assert_eq!(n.tree_bytes(0), 0);
    assert_eq!(romfs::entries(&n, 0).len(), K as usize);
    // the same chain with every link listed twice: a DAG of 2^39 paths
    let e = open(folders_only(&chain(true)))
        .err()
        .expect("a DAG of folders was accepted")
        .to_string();
    assert!(e.contains("listed by more than one folder"), "{e}");
    // two different parents of one folder (0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3)
    let e = open(folders_only(&[vec![1, 2], vec![3], vec![3], vec![]]))
        .err()
        .expect("a folder with two parents was accepted")
        .to_string();
    assert!(
        e.contains("folder \"d\" (index 3) is listed by more than one folder"),
        "{e}"
    );
    // and the path the review names, the cover (icon_of -> app_package -> scan -> tree_bytes), is an error, not 2^39 steps
    assert!(cover_of(folders_only(&chain(true))).is_err());
}

/// Review 2026-11-20 §5.15: an ExeFS folder that differs is replaced by a rename of the new one, the old one moved aside first, never deleted
/// before the new one stands. A stale folder the delete cannot empty (a read-only subfolder) used to be half deleted, then the rename over it
/// failed (not empty): the extraction failed and the old ExeFS was damaged. It is now replaced, and the new files are all there.
#[test]
fn an_exefs_folder_is_replaced_by_a_rename_never_deleted_first() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmp("exefs-replace");
    let p = container(&d, "g.nup", &[base(APP, small_tree())]);
    let n = Nup::open(&p).unwrap();
    let k = n.packages[0].clone();
    let dst = d.join("ExeFS");
    std::fs::create_dir_all(dst.join("locked")).unwrap();
    std::fs::write(dst.join("main"), b"stale").unwrap();
    std::fs::write(dst.join("locked").join("f"), b"x").unwrap();
    std::fs::set_permissions(dst.join("locked"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let r = ibuilder_nup::extract::exefs(&n, &k, &dst, &opts_with(1));
    // put the permissions back before any assertion, so the folder can be cleaned up
    for f in [dst.join("locked"), d.join("ExeFS.old").join("locked")] {
        let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755));
    }
    r.expect("the ExeFS was not replaced");
    let mut names: Vec<String> = std::fs::read_dir(&dst)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, vec!["main", "main.npdm"]);
    assert_eq!(std::fs::read(dst.join("main")).unwrap(), bytes(9, 3000));
    assert!(!d.join("ExeFS.partial").exists());
    // and an ExeFS that is already right is kept as it is (same inode)
    use std::os::unix::fs::MetadataExt;
    let ino = std::fs::metadata(dst.join("main")).unwrap().ino();
    ibuilder_nup::extract::exefs(&n, &k, &dst, &opts_with(1)).unwrap();
    assert_eq!(std::fs::metadata(dst.join("main")).unwrap().ino(), ino);
    let _ = std::fs::remove_dir_all(&d);
}
