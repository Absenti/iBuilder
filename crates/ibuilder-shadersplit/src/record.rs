//! Which readers a split needed, so that a newer split re-reads only the games it reads differently (the pipeline's targeted re-split; no Python counterpart).
//!
//! [`SPLIT_REV`](crate::SPLIT_REV) is the GLOBAL revision: a change that can alter every game's output (the merge, the record cut, the generic scan) bumps it, and every library built on
//! the phone is split again. A change to one reader bumps that reader's revision in [`READERS`] instead. The split writes a record (`SplitOptions::record`):
//!   * `revs`: the revision of every reader this build has;
//!   * `uses`: the readers the game needed: a top-level file the reader recognises ([`classify`]: its magic, its layout, its name), or a shader label that carries the reader's tag;
//!   * `unknown`: the files no reader recognised, as `<extension>#<first four bytes in hex>` (at most [`MAGICS_PER_EXT`] per extension, then `<extension>#*`), so that a reader added later
//!     can tell whether it would read one of them ([`Reader::claims`]).
//!
//! A library is split again when the global revision differs, when a reader it uses has another revision now, or when a reader whose revision moved claims one of its unknown kinds.
//! A library from before records existed is taken to have [`LEGACY`]'s revisions, and its game's files are classified again ([`probe`]) to know what it uses.
//! Limits: a file is classified by what it is at the top of the RomFS; a container nested in another one (a SARC in a zstd file) is seen only through the labels of the shaders it gave. A
//! reader that may read files it does not recognise at the top must say so in its claims, or its change must bump the global revision.

use crate::image::walk_root;
use crate::scan::FileRef;
use crate::source::Source;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One reader family: its name in the record, its revision, the kinds of unknown files it would read (`.ext`, or `#<hex prefix of the first bytes>`), and the tags its shader labels carry
pub struct Reader {
    pub name: &'static str,
    pub rev: u32,
    pub claims: &'static [&'static str],
    pub tags: &'static [&'static str],
}

/// The reader families and their revisions. Bump a reader's `rev` when a change can alter what it finds; add a family with rev 1 (a library recorded without it is at 0).
/// History: every family existed at SPLIT_REV 3 with rev 1, except: cpk and d3cpk (new: CRI CPK and Diablo III packages, cpk.rs), ea (new: EA's encrypted shader archives, nxnative
/// cb6eb28b), falcom, mvgl, nxsd and unreal-pak (new in nxnative 930e9195, 131bd013 + 8cc63cc9, a63ca651, the Unreal paks of uepak.py), casc, cra, lspk, pdat and rpf6 (lspk.py, casc.py,
/// cra.py with scan_blob's NARC branch, pdat_jobs, rpf6.py: those games ended with "unsupported container" before), smash-arc and trinity (new: smash_arc.rs, trinity.rs),
/// phyre 2 (930e9195: whole GLSLC outputs in an RYHP container that holds no Phyre program),
/// snowdrop 2 (e74c3fa7: each block of a bare zlib cache must decode to exactly 64 KB, the last one to the rest),
/// unreal-pak 2 (IoStore containers: UE5 shader libraries and export bundles beside an inline pak; paks with an encrypted index, the AES key from main),
/// frostbite and ttdat (new: `frostbite.py` 682fdd26, `ttdat.py` 9f834e41 + 5cd38d39). 9f834e41 also gave main's loose control sections two more rules (`raw_shaders`: a code copy built by
/// address, a unique adjacent code area): the TT Games form. Its known game re-splits through ttdat (its DATs are in `uses`); generic stays at 1, so a library of a game without TT DATs whose
/// main has such loose sections keeps its tables until it is split again for another reason (bumping generic would split every library again).
/// nxnative 033793f0 sync: bahs, mtarc, nxcx, rage-wad, redengine, sharc, telltale, vbf (new); ipk 2 (version 5 archives), ue3 2 (zero padding after the last chunk), zip 2 (Star Fox's
/// chained-LZ4 members, refused before). The zstd reader now decodes every frame of a file (6aaf73ba): a file of several frames was cut after the first; it has no family of its own, and a
/// game whose RomFS has such a file is split again only through another reason.
pub const READERS: &[Reader] = &[
    Reader {
        name: "apf",
        rev: 1,
        claims: &["#46505556"],
        tags: &[],
    },
    // agl BAHS files (5df3dbe8: a validated binary array drops stray descriptors); they ship in SARC / Yaz0 / zstd packs, whose kinds it claims
    Reader {
        name: "bahs",
        rev: 1,
        claims: &["#42414853", "#53415243", "#59617a30", "#28b52ffd"],
        tags: &["#BAHS@"],
    },
    Reader {
        name: "casc",
        rev: 1,
        claims: &[".psv"],
        tags: &[],
    },
    // CRI CPK archives (cpk.py), new in this port's nxnative sync
    Reader {
        name: "cpk",
        rev: 1,
        claims: &["#43504b20"],
        tags: &[],
    },
    Reader {
        name: "cra",
        rev: 1,
        claims: &["#4352412e", "#4e415243"],
        tags: &[],
    },
    // Diablo III's packages (d3cpk.py), new in this port's nxnative sync
    Reader {
        name: "d3cpk",
        rev: 1,
        claims: &["#d4c3b2a1"],
        tags: &["#D3@"],
    },
    // EA's BIG4 / EASF / chunk containers / NXSHADER (easf.rs, nxnative cb6eb28b): BIG4, EASF, NXSH(ADER), chun(kzip/zstd/lzma/kref)
    Reader {
        name: "ea",
        rev: 1,
        claims: &["#42494734", "#45415346", "#4e585348", "#6368756e"],
        tags: &["#NXSHADER"],
    },
    Reader {
        name: "falcom",
        rev: 1,
        claims: &[".pkg", ".pka", "#0dcff77f"],
        tags: &[],
    },
    // Frostbite's superbundle tables: every .toc starts with the toc magic (the .cas files they name are taken with them)
    Reader {
        name: "frostbite",
        rev: 1,
        claims: &["#00d1ce01"],
        tags: &[],
    },
    // everything no other family names: the blind scan of plain files and main, BNSH, BAHS, GLSLC, SARC, Yaz0, zstd and the other readers of scan_blob
    Reader {
        name: "generic",
        rev: 1,
        claims: &[],
        tags: &[],
    },
    Reader {
        name: "ipk",
        rev: 2,
        claims: &["#50ec12ba"],
        tags: &[],
    },
    Reader {
        name: "kit",
        rev: 1,
        claims: &[".kit"],
        tags: &[],
    },
    Reader {
        name: "kpka",
        rev: 1,
        claims: &["#4b504b41"],
        tags: &[],
    },
    Reader {
        name: "lspk",
        rev: 1,
        claims: &[".pak"],
        tags: &["#BSHD#"],
    },
    // MT Framework ARC v7 / v9 and SPK packages, headed or contiguous (c931e584)
    Reader {
        name: "mtarc",
        rev: 1,
        claims: &["#41524300"],
        tags: &["#MTSPK@"],
    },
    Reader {
        name: "mvgl",
        rev: 1,
        claims: &["#4d444231", ".mvgl", ".shad"],
        tags: &["#NXSH@"],
    },
    // NXCX texture archives searched in chunks (0ec204f5)
    Reader {
        name: "nxcx",
        rev: 1,
        claims: &["#4e584358"],
        tags: &[],
    },
    Reader {
        name: "nxsd",
        rev: 1,
        claims: &["#4e585344"],
        tags: &["#NXSD@"],
    },
    Reader {
        name: "pdat",
        rev: 1,
        claims: &["#50444154"],
        tags: &["#PMAP@"],
    },
    Reader {
        name: "phyre",
        rev: 2,
        claims: &["#52594850", ".phyre"],
        tags: &["#PHYRE@"],
    },
    // RAGE WAD v1 out.wad.nx, VFP GLSLC arrays (19c5379d)
    Reader {
        name: "rage-wad",
        rev: 1,
        claims: &["#57414401"],
        tags: &["#VFP@"],
    },
    // REDengine 3 RDHS material caches and FZLC static caches (a4c82979)
    Reader {
        name: "redengine",
        rev: 1,
        claims: &["#465a4c43", ".cache"],
        tags: &[],
    },
    Reader {
        name: "rpf6",
        rev: 1,
        claims: &["#52504636"],
        tags: &[],
    },
    // GIANTS SHARC v11, raw LZMA1 members (f3950c50)
    Reader {
        name: "sharc",
        rev: 1,
        claims: &["#52414853"],
        tags: &[],
    },
    // Super Smash Bros. Ultimate's data.arc (smash_arc.rs): its magic; the extension is MT Framework's ARC archives' too
    Reader {
        name: "smash-arc",
        rev: 1,
        claims: &["#10325476"],
        tags: &[],
    },
    Reader {
        name: "snowdrop",
        rev: 2,
        claims: &["#42455247"],
        tags: &["#SNOWDROP@"],
    },
    // Telltale .ttarch2 ZCTT / NCTT archives (c714bb92)
    Reader {
        name: "telltale",
        rev: 1,
        claims: &["#5a435454", "#4e435454"],
        tags: &["#TTFX@"],
    },
    // Game Freak's Trinity packs (trinity.rs): data.trpfs ('ONEPACK') and its descriptor data.trpfd
    Reader {
        name: "trinity",
        rev: 1,
        claims: &["#4f4e4550", ".trpfs", ".trpfd"],
        tags: &[".trpfs#"],
    },
    // TT Games' CC4 DATs have no magic at their start: a DAT is classified by its index. No claim: a `.dat#...` kind no reader recognised is not one (a claim of `.dat` would split
    // every library with any unknown .dat again)
    Reader {
        name: "ttdat",
        rev: 1,
        claims: &[],
        tags: &[],
    },
    Reader {
        name: "ue3",
        rev: 2,
        claims: &["#424d5347", "#c1832a9e"],
        tags: &["#UE3@"],
    },
    Reader {
        name: "unity",
        rev: 1,
        claims: &["#556e6974"],
        tags: &[],
    },
    Reader {
        name: "unreal-pak",
        rev: 2,
        claims: &[".pak", ".ushaderbytecode", ".utoc", ".ucas"],
        tags: &["#UE"],
    },
    // Virtuos VBF archives (vbf.py), refused before
    Reader {
        name: "vbf",
        rev: 1,
        claims: &["#5352594b"],
        tags: &[],
    },
    Reader {
        name: "zip",
        rev: 2,
        claims: &["#504b0304", "#565a4950"],
        tags: &[],
    },
];

/// The revisions a library split before records existed (SPLIT_REV 3, no `split-revs` row) is taken to have
pub const LEGACY: &[(&str, u32)] = &[
    ("apf", 1),
    ("generic", 1),
    ("ipk", 1),
    ("kit", 1),
    ("kpka", 1),
    ("phyre", 1),
    ("snowdrop", 1),
    ("ue3", 1),
    ("unity", 1),
    ("zip", 1),
];

/// how many first-byte kinds of one extension the record keeps before it says `<ext>#*`
pub const MAGICS_PER_EXT: usize = 8;

/// `apf=1 falcom=1 ...`: this build's revisions, sorted by name
pub fn revisions() -> String {
    let m: BTreeMap<&str, u32> = READERS.iter().map(|r| (r.name, r.rev)).collect();
    m.iter()
        .map(|(n, r)| format!("{n}={r}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// [`LEGACY`] in the same form
pub fn legacy_revisions() -> String {
    let m: BTreeMap<&str, u32> = LEGACY.iter().copied().collect();
    m.iter()
        .map(|(n, r)| format!("{n}={r}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// What one split needed: the reader families it used and the kinds of the files no reader recognised
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub uses: BTreeSet<String>,
    /// extension → first-byte kinds (`None`: more than [`MAGICS_PER_EXT`])
    pub unknown: BTreeMap<String, Option<BTreeSet<String>>>,
}

impl Record {
    pub fn add_unknown(&mut self, ext: &str, magic: &str) {
        let e = self
            .unknown
            .entry(ext.to_string())
            .or_insert_with(|| Some(BTreeSet::new()));
        if let Some(set) = e {
            set.insert(magic.to_string());
            if set.len() > MAGICS_PER_EXT {
                *e = None;
            }
        }
    }

    /// the families whose tags this shader label carries
    pub fn add_label(&mut self, label: &str) {
        for r in READERS {
            if r.tags.iter().any(|t| has_tag(label, t)) {
                self.uses.insert(r.name.to_string());
            }
        }
    }

    pub fn merge(&mut self, o: &Record) {
        self.uses.extend(o.uses.iter().cloned());
        for (e, kinds) in &o.unknown {
            match kinds {
                None => {
                    self.unknown.insert(e.clone(), None);
                }
                Some(k) => {
                    for m in k {
                        self.add_unknown(e, m);
                    }
                }
            }
        }
    }

    /// the `unknown` row: `<ext>#<magic>` words, sorted
    pub fn unknown_text(&self) -> String {
        let mut v = Vec::new();
        for (e, kinds) in &self.unknown {
            match kinds {
                None => v.push(format!("{e}#*")),
                Some(k) => v.extend(k.iter().map(|m| format!("{e}#{m}"))),
            }
        }
        v.join(" ")
    }

    /// The record file: `revs`, `uses` and `unknown` rows (tab-separated), this build's revisions
    pub fn text(&self) -> String {
        format!(
            "revs\t{}\nuses\t{}\nunknown\t{}\n",
            revisions(),
            self.uses.iter().cloned().collect::<Vec<_>>().join(" "),
            self.unknown_text()
        )
    }
}

/// `#UE` must not match the Unreal Engine 3 reader's `#UE3@`
fn has_tag(label: &str, tag: &str) -> bool {
    if tag != "#UE" {
        return label.contains(tag);
    }
    label
        .match_indices("#UE")
        .any(|(i, _)| !label[i + 3..].starts_with("3@"))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `os.path.splitext`'s extension of the last path component, lower case (`""` when there is none)
fn ext_of(rel: &str) -> String {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    let trimmed = base.trim_start_matches('.');
    match trimmed.rfind('.') {
        Some(i) => trimmed[i..].to_ascii_lowercase(),
        None => String::new(),
    }
}

/// A file of a CASC folder: `build_key.txt`, a manifest, an archive `<chunk>-<archive>.data`
fn casc_name(lower: &str) -> bool {
    let base = lower.rsplit('/').next().unwrap_or(lower);
    base == "build_key.txt"
        || base.ends_with(".psv")
        || (base.len() == 16 && base.ends_with(".data") && base.as_bytes()[2] == b'-')
}

/// What a top-level file is to the split: `Ok(families)` the readers that take it, `Err((ext, magic))` none (an unknown kind)
pub fn classify_source(rel: &str, src: &dyn Source) -> Result<Vec<&'static str>, (String, String)> {
    let len = src.len();
    let head = src.read(0, 0x60).unwrap_or_default();
    let h = |a: usize, b: usize| head.get(a..b.min(head.len())).unwrap_or(&[]);
    let lower = rel.to_ascii_lowercase();
    let mut out: Vec<&'static str> = Vec::new();
    let h4 = h(0, 4);
    if crate::easf::recognizes(h(0, 8)) {
        out.push("ea");
    } else if h4 == crate::cpk::CPK_MAGIC && lower.ends_with(".cpk") {
        out.push("cpk");
    } else if lower.ends_with(".cpk") && crate::cpk::is_d3cpk(h(0, 8)) {
        out.push("d3cpk");
    } else if h(0, 8) == crate::smash_arc::MAGIC.as_slice() {
        out.push("smash-arc");
    } else if h(0, 8) == crate::trinity::MAGIC.as_slice() || lower.ends_with(".trpfd") {
        out.push("trinity");
    } else if h4 == crate::nxsd::MAGIC {
        out.push("nxsd");
    } else if h4 == [0x50, 0xec, 0x12, 0xba] {
        out.push("ipk");
    } else if h4 == crate::apf::MAGIC {
        out.push("apf");
    } else if h4 == crate::telltale::TTCZ || h4 == crate::telltale::TTCN {
        out.push("telltale");
    } else if h4 == crate::sharc::MAGIC {
        out.push("sharc");
    } else if h4 == crate::redengine::FZLC
        || (len >= 16
            && (h(8, 9) == [1] || lower.ends_with("shadernx.cache"))
            && src
                .read(len - 8, 4)
                .is_ok_and(|t| t == crate::redengine::RDHS))
    {
        out.push("redengine");
    } else if h4 == crate::rage_wad::MAGIC
        && lower.rsplit('/').next() == Some(crate::rage_wad::NAME)
    {
        out.push("rage-wad");
    } else if crate::nxcx::is_nxcx(h(0, 16)) {
        out.push("nxcx");
    } else if h4 == b"SRYK" {
        out.push("vbf");
    } else if h4 == crate::archive::MTARC {
        out.push("mtarc");
    } else if h4 == b"BAHS" {
        out.push("bahs");
    } else if h4 == b"BERG" {
        out.push("snowdrop");
    } else if h4 == b"BMSG" || h4 == [0xc1, 0x83, 0x2a, 0x9e] {
        out.push("ue3");
    } else if h4 == crate::frostbite::TOC_MAGIC {
        out.push("frostbite");
    } else if h4 == crate::falcom::PKA_MAGIC {
        out.push("falcom");
    } else if h4 == crate::mvgl::MAGIC {
        out.push("mvgl");
    } else if h4 == b"KPKA" && lower.ends_with(".pak") {
        out.push("kpka");
    } else if lower.ends_with(".pak")
        && len >= 8
        && src
            .read(len - 8, 8)
            .is_ok_and(|t| t.len() == 8 && &t[4..] == crate::lspk::MAGIC)
    {
        out.push("lspk");
    } else if h4 == crate::pdat::MAGIC {
        out.push("pdat");
    } else if h4 == crate::rpf6::MAGIC && len >= 16 {
        out.push("rpf6");
    } else if h4 == crate::cra::MAGIC || h4 == b"NARC" {
        out.push("cra");
    } else if casc_name(&lower) {
        out.push("casc");
    } else if (lower.ends_with(".pak") && crate::uepak::probe(src).is_some())
        || (lower.ends_with(".utoc") && h(0, 16) == crate::uestore::TOC_MAGIC.as_slice())
    {
        out.push("unreal-pak");
    } else if lower.ends_with(".kit") {
        out.push("kit");
        out.push("phyre"); // Unravel TWO's KIT shards hold Phyre effects
    } else if h4 == b"PK\x03\x04" || h4 == b"VZIP" {
        out.push("zip");
    } else if h4 == b"RYHP" || h(8, 12) == crate::ctl::GFX_MAGIC.as_slice() {
        out.push("phyre");
    } else if h(0, 8) == b"UnityFS\0"
        || (h(8, 11) == [0, 0, 0] && crate::unity::is_serialized(h(0, 48), len))
    {
        out.push("unity");
    } else if h4 == [0, 0, 0, 0]
        && len >= 8 + 0x50
        && matches!(crate::falcom::falcom_pkg(src), Ok(Some(_)))
    {
        out.push("falcom");
    }
    // a TT Games DAT is found by the index its first word points at (`ttdat.index_header`: one it recognises and cannot read is still taken by its reader)
    if lower.ends_with(".dat") && !matches!(crate::ttdat::index_header(src), Ok(None)) {
        out.push("ttdat");
    }
    if out.is_empty() {
        Err((ext_of(rel), hex(h4)))
    } else {
        Ok(out)
    }
}

/// [`classify_source`] of every file, into a record (an unreadable file counts as an unknown kind `<ext>#`). The reads are spread over a few threads: a RomFS image of 186,583
/// files (Ocarina of Time, 26 GB) costs one random read each, about 11 s on one thread on an M1 Pro
pub fn classify_files(files: &[FileRef]) -> Record {
    fn one(files: &[FileRef]) -> Record {
        let mut r = Record::default();
        for f in files {
            match f.loc.open() {
                Ok(src) => match classify_source(&f.rel, &src) {
                    Ok(fams) => r.uses.extend(fams.into_iter().map(String::from)),
                    Err((e, m)) => {
                        r.uses.insert("generic".into());
                        r.add_unknown(&e, &m);
                    }
                },
                Err(_) => r.add_unknown(&ext_of(&f.rel), ""),
            }
        }
        r
    }
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8);
    if files.len() < 4096 || threads == 1 {
        return one(files);
    }
    let parts: Vec<Record> = std::thread::scope(|sc| {
        let hs: Vec<_> = files
            .chunks(files.len().div_ceil(threads))
            .map(|c| sc.spawn(move || one(c)))
            .collect();
        hs.into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });
    let mut r = Record::default();
    for p in &parts {
        r.merge(p);
    }
    r
}

/// For a library split before records existed: its game's files classified again (the RomFS, the DLC roots) and the labels of the shader map it has (column 5)
pub fn probe(romfs: &Path, dlc: &[&Path], shader_map: Option<&Path>) -> Result<Record, String> {
    let mut files = Vec::new();
    for (i, root) in std::iter::once(romfs)
        .chain(dlc.iter().copied())
        .enumerate()
    {
        for e in walk_root(root)? {
            files.push(FileRef {
                root: i,
                rel: e.rel,
                loc: e.loc,
            });
        }
    }
    let mut r = classify_files(&files);
    r.uses.insert("generic".into()); // main is always scanned by the generic readers
    if let Some(m) = shader_map
        && let Ok(text) = std::fs::read_to_string(m)
    {
        for line in text.lines() {
            if let Some(label) = line.splitn(5, '\t').nth(4) {
                r.add_label(label);
            }
        }
    }
    Ok(r)
}

/// `probe`, as the record file's text (the rows the pipeline keeps in `pack.txt`)
pub fn probe_text(
    romfs: &Path,
    dlc: &[&Path],
    shader_map: Option<&Path>,
) -> Result<String, String> {
    probe(romfs, dlc, shader_map).map(|r| r.text())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_and_kinds() {
        let mut r = Record::default();
        r.add_label("/a.pak#Game/M.uexp#UE3a0");
        assert!(r.uses.contains("unreal-pak"));
        let mut r = Record::default();
        r.add_label("/Engine/Cache.bin#UE3@1c0");
        assert_eq!(r.uses.iter().collect::<Vec<_>>(), ["ue3"]);
        for i in 0..20 {
            r.add_unknown(".bin", &format!("{i:08x}"));
        }
        r.add_unknown("", "00000000");
        assert_eq!(r.unknown_text(), "#00000000 .bin#*");
        assert!(r.text().starts_with(
            "revs\tapf=1 bahs=1 casc=1 cpk=1 cra=1 d3cpk=1 ea=1 falcom=1 frostbite=1 generic=1 ipk=2 kit=1 kpka=1 lspk=1 mtarc=1 mvgl=1 nxcx=1 nxsd=1 pdat=1 phyre=2 rage-wad=1 redengine=1 rpf6=1 sharc=1 smash-arc=1 snowdrop=2 telltale=1 trinity=1 ttdat=1 ue3=2 unity=1 unreal-pak=2 vbf=1 zip=2"
        ));
        assert_eq!(ext_of("Data/x.Fx.PHYRE"), ".phyre");
        assert_eq!(ext_of("Data/.hidden"), "");
    }

    #[test]
    fn every_legacy_family_exists_and_names_are_unique() {
        let names: BTreeSet<&str> = READERS.iter().map(|r| r.name).collect();
        assert_eq!(names.len(), READERS.len());
        for (n, _) in LEGACY {
            assert!(names.contains(n), "{n}");
        }
    }
}
