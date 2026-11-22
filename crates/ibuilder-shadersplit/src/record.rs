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
/// History: every family existed at SPLIT_REV 3 with rev 1, except: falcom, mvgl, nxsd and unreal-pak (new in nxnative 930e9195, 131bd013 + 8cc63cc9, a63ca651, the Unreal paks
/// of uepak.py), phyre 2 (930e9195: whole GLSLC outputs in an RYHP container that holds no Phyre program),
/// snowdrop 2 (e74c3fa7: each block of a bare zlib cache must decode to exactly 64 KB, the last one to the rest).
pub const READERS: &[Reader] = &[
    Reader {
        name: "apf",
        rev: 1,
        claims: &["#46505556"],
        tags: &[],
    },
    Reader {
        name: "falcom",
        rev: 1,
        claims: &[".pkg", ".pka", "#0dcff77f"],
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
        rev: 1,
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
        name: "mvgl",
        rev: 1,
        claims: &["#4d444231", ".mvgl", ".shad"],
        tags: &["#NXSH@"],
    },
    Reader {
        name: "nxsd",
        rev: 1,
        claims: &["#4e585344"],
        tags: &["#NXSD@"],
    },
    Reader {
        name: "phyre",
        rev: 2,
        claims: &["#52594850", ".phyre"],
        tags: &["#PHYRE@"],
    },
    Reader {
        name: "snowdrop",
        rev: 2,
        claims: &["#42455247"],
        tags: &["#SNOWDROP@"],
    },
    Reader {
        name: "ue3",
        rev: 1,
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
        rev: 1,
        claims: &[".pak", ".ushaderbytecode"],
        tags: &["#UE"],
    },
    Reader {
        name: "zip",
        rev: 1,
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

/// What a top-level file is to the split: `Ok(families)` the readers that take it, `Err((ext, magic))` none (an unknown kind)
pub fn classify_source(rel: &str, src: &dyn Source) -> Result<Vec<&'static str>, (String, String)> {
    let len = src.len();
    let head = src.read(0, 0x60).unwrap_or_default();
    let h = |a: usize, b: usize| head.get(a..b.min(head.len())).unwrap_or(&[]);
    let lower = rel.to_ascii_lowercase();
    let mut out: Vec<&'static str> = Vec::new();
    let h4 = h(0, 4);
    if h4 == crate::nxsd::MAGIC {
        out.push("nxsd");
    } else if h4 == [0x50, 0xec, 0x12, 0xba] {
        out.push("ipk");
    } else if h4 == crate::apf::MAGIC {
        out.push("apf");
    } else if h4 == b"BERG" {
        out.push("snowdrop");
    } else if h4 == b"BMSG" || h4 == [0xc1, 0x83, 0x2a, 0x9e] {
        out.push("ue3");
    } else if h4 == crate::falcom::PKA_MAGIC {
        out.push("falcom");
    } else if h4 == crate::mvgl::MAGIC {
        out.push("mvgl");
    } else if h4 == b"KPKA" && lower.ends_with(".pak") {
        out.push("kpka");
    } else if lower.ends_with(".pak") && crate::uepak::probe(src).is_some() {
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
        assert!(r.text().starts_with("revs\tapf=1 falcom=1 generic=1"));
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
