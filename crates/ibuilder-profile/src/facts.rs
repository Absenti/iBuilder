//! `profile_guess.facts_of(exefs, romfs)`: what the profile rules need from a game's code modules and RomFS.
use crate::error::{Error, Result};
use crate::nso;
use crate::romfs::romfs_for_each;
use crate::search;
use crate::syms::{self, area};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// `(marker bytes, name)`: `MARKERS` in profile_guess.py.
pub const MARKERS: [(&[u8], &str); 15] = [
    (b"nvnBootstrapLoader", "NVN"),
    (b"vkCreateInstance", "Vulkan"),
    (b"eglGetProcAddress", "EGL / OpenGL"),
    (b"global-metadata.dat", "Unity IL2CPP"),
    (b"UnityEngine", "Unity"),
    (b"sead::", "Nintendo sead"),
    (b"agl::", "Nintendo agl"),
    (b"gsys::", "Nintendo gsys"),
    (b"Unreal", "Unreal"),
    (b"via.render.", "RE Engine"),
    (b"Havok", "Havok"),
    (b"CRIWARE", "CRIWARE"),
    (b"Wwise", "Wwise"),
    (b"MonoGame.Framework", "MonoGame"),
    (b"FMOD mixer", "FMOD"),
];

/// `MAGICS`: RomFS file header magics, in match order.
const MAGICS: &[(&[u8], &str)] = &[
    (b"UnityFS\0", "UnityFS"),
    (b"UnityWeb", "UnityWeb"),
    (b"UnityRaw", "UnityRaw"),
    (b"BNSH", "BNSH"),
    (b"BAHS", "BAHS"),
    (b"SARC", "SARC"),
    (b"Yaz0", "Yaz0"),
    (&[0x28, 0xb5, 0x2f, 0xfd], "zstd"),
    (b"FRES", "bfres"),
    (b"BNTX", "BNTX"),
    (b"BWAV", "BWAV"),
    (b"FSAR", "BFSAR"),
    (b"CPK ", "CRI CPK"),
    (b"CRID", "CRI USM"),
    (b"@UTF", "CRI UTF"),
    (b"AKPK", "Wwise pck"),
    (b"BKHD", "Wwise bnk"),
    (b"FSB5", "FMOD"),
    (b"\x89PNG", "PNG"),
    (b"RIFF", "RIFF"),
    (b"OggS", "Ogg"),
    (b"PK\x03\x04", "zip"),
    (b"NSO0", "NSO"),
    (&[0x03, 0x02, 0x23, 0x07], "SPIR-V"),
    (b"ONEPACK\0", "Trinity pack"),
    (b"ARC\0", "MT Framework ARC"),
    (b"SCNE", "Bezel Engine archive"),
    (b"XNB", "XNB"),
    (b"pkzl", "PlatinumGames PKZ"),
    (b"KPKA", "RE Engine PAK"),
];

const PACKED: [&str; 17] = [
    "UnityFS",
    "UnityWeb",
    "Yaz0",
    "zstd",
    "BWAV",
    "CRI CPK",
    "CRI USM",
    "Wwise pck",
    "FMOD",
    "PNG",
    "Ogg",
    "zip",
    "Trinity pack",
    "MT Framework ARC",
    "Bezel Engine archive",
    "PlatinumGames PKZ",
    "RE Engine PAK",
];

const GLSLC_MAGIC: &[u8] = &[0x91, 0x68, 0x86, 0x19];

/// What `facts_of` records about one module.
#[derive(Clone, Debug)]
pub struct ModFacts {
    pub bid: String,
    pub sizes: Vec<u64>,
    pub bss: u64,
    /// The patch area goes before the base (patch_nso.py FAR): the bss counts as used memory (hle_svc.c big_bss_bytes).
    pub far: bool,
    pub imports: BTreeSet<String>,
    pub exports: BTreeSet<String>,
    pub comps: BTreeSet<String>,
    pub kind: String,
}

/// Insertion-ordered counter (Python's `Counter`): `most_common` keeps first-seen order among equal counts.
#[derive(Clone, Debug, Default)]
pub struct Counter {
    items: Vec<(String, u64)>,
}

impl Counter {
    pub fn add(&mut self, key: &str, n: u64) {
        if let Some(s) = self.items.iter_mut().find(|(k, _)| k == key) {
            s.1 += n;
        } else {
            self.items.push((key.to_string(), n));
        }
    }
    pub fn get(&self, key: &str) -> u64 {
        self.items.iter().find(|(k, _)| k == key).map_or(0, |s| s.1)
    }
    pub fn iter(&self) -> impl Iterator<Item = &(String, u64)> {
        self.items.iter()
    }
    pub fn most_common(&self, n: usize) -> Vec<(String, u64)> {
        let mut v = self.items.clone();
        v.sort_by_key(|b| std::cmp::Reverse(b.1)); // stable: equal counts keep insertion order
        v.truncate(n);
        v
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// `F['romfs']`.
#[derive(Clone, Debug, Default)]
pub struct RomfsFacts {
    pub n: u64,
    pub bytes: u64,
    pub tops: Counter,
    pub topb: Counter,
    pub exts: Counter,
    pub extb: Counter,
    pub extc: Counter,
    pub mags: Counter,
    pub magb: Counter,
    pub dirs: Counter,
    /// `(size, path)` of the largest file (`(0, '')` without files; ties by path).
    pub biggest: (u64, String),
    pub root: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// In file-name order (the Python dict's insertion order: `sorted(exefs.iterdir())`).
    pub mods: Vec<(String, ModFacts)>,
    pub ounce: bool,
    pub markers: BTreeMap<String, Vec<String>>,
    pub sdkver: BTreeSet<String>,
    pub comps: BTreeSet<String>,
    pub unity: BTreeSet<String>,
    pub glslc: usize,
    pub glsl_src: usize,
    pub gl_names: usize,
    pub gl_src: usize,
    pub sysres: u64,
    pub program: Option<u64>,
    pub sdk: u64,
    pub main_markers: BTreeSet<String>,
    pub romfs: RomfsFacts,
}

impl Facts {
    pub fn module(&self, name: &str) -> Option<&ModFacts> {
        self.mods.iter().find(|(n, _)| n == name).map(|(_, m)| m)
    }
}

/// `kind(m)`: a short name for what a module is: its NintendoSDK component, GLSLC, or the `nn::` namespace most
/// of its exports are in. (Ties between namespaces are broken by first-seen order in sorted symbol order; the
/// Python iterates a hash-ordered set, so a tie is arbitrary there.)
pub fn kind(comps: &BTreeSet<String>, exports: &BTreeSet<String>) -> String {
    if let Some(c) = comps.iter().next() {
        return c.clone();
    }
    if exports.iter().any(|s| s.starts_with("glslc")) {
        return "GLSLC".into();
    }
    let mut areas = Counter::default();
    for s in exports {
        areas.add(&area(s), 1);
    }
    areas
        .most_common(1)
        .into_iter()
        .next()
        .map_or("?".into(), |a| a.0)
}

/// `npdm(path)`: (system resource size, program ID) from `main.npdm`; `(0, None)` without the file.
pub fn npdm(path: &Path) -> Result<(u64, Option<u64>)> {
    if !path.exists() {
        return Ok((0, None));
    }
    let d = std::fs::read(path)?;
    let a = search::find(&d, b"ACI0", 0);
    let sysres = if d.starts_with(b"META") {
        u32::from_le_bytes(
            d.get(0x14..0x18)
                .ok_or_else(|| Error::Format("main.npdm is cut short".into()))?
                .try_into()
                .unwrap(),
        ) as u64
    } else {
        0
    };
    let program = match a {
        Some(a) => Some(u64::from_le_bytes(
            d.get(a + 0x10..a + 0x18)
                .ok_or_else(|| Error::Format("main.npdm is cut short".into()))?
                .try_into()
                .unwrap(),
        )),
        None => None,
    };
    Ok((sysres, program))
}

struct Scanned {
    name: String,
    m: ModFacts,
    sdkver: Vec<String>,
    ounce: bool,
    comps: Vec<String>,
    marker_hits: Vec<&'static str>,
    main: Option<MainFacts>,
}

struct MainFacts {
    unity: Vec<String>,
    glslc: usize,
    glsl_src: usize,
    gl_names: usize,
    gl_src: usize,
}

fn scan_module(path: &Path, name: &str) -> Result<Scanned> {
    let module = nso::load_file(path)?;
    let img = &module.image;
    let bid = module.bid_hex();
    let sizes: Vec<u64> = module.segments.iter().map(|s| s.1).collect();
    let bss = module.bss as u64;
    let syms = syms::dynsyms(img)?;
    // where patch_nso.py would append the patch area (NEAR_PATCH_OFF)
    let end = ((((img.len() as u64 + bss + 0x3FFF) & !0x3FFF) + 0x4000 + 0xFFFF) & !0xFFFF) as u64;
    let imports: BTreeSet<String> = syms
        .iter()
        .filter(|s| !s.defined)
        .map(|s| s.name.clone())
        .collect();
    let exports: BTreeSet<String> = syms
        .iter()
        .filter(|s| s.defined && (s.bind == 1 || s.bind == 2))
        .map(|s| s.name.clone())
        .collect();
    let comps: BTreeSet<String> = search::nintendo_sdk_components(img)
        .into_iter()
        .map(|c| String::from_utf8_lossy(&c).into_owned())
        .collect();
    let sdkver: Vec<String> = search::sdk_versions(img)
        .into_iter()
        .map(|c| String::from_utf8_lossy(&c).into_owned())
        .collect();
    let ounce = name == "sdk" && search::contains(img, b"spec: Ounce");
    let mw: Vec<String> = search::sdk_mw_components(img)
        .into_iter()
        .map(|c| String::from_utf8_lossy(&c).into_owned())
        .collect();
    // the independent byte scans of one module in parallel (main is 75 MB)
    let marker_hits: Vec<&'static str> = MARKERS
        .par_iter()
        .filter(|(pat, _)| search::contains(img, pat))
        .map(|(_, n)| *n)
        .collect();
    let main = (name == "main").then(|| {
        let (a, b, c, d, e) = {
            let ((unity, glslc), (glsl_src, (gl_names, gl_src))) = rayon::join(
                || {
                    rayon::join(
                        || search::unity_versions(img),
                        || search::count(img, GLSLC_MAGIC),
                    )
                },
                || {
                    rayon::join(
                        || search::glsl_sources(img),
                        || {
                            rayon::join(
                                || search::count(img, b"glShaderSource"),
                                || search::count_gl_src(img),
                            )
                        },
                    )
                },
            );
            (unity, glslc, glsl_src, gl_names, gl_src)
        };
        MainFacts {
            unity: a
                .into_iter()
                .map(|v| String::from_utf8_lossy(&v).into_owned())
                .collect(),
            glslc: b,
            glsl_src: c,
            gl_names: d,
            gl_src: e,
        }
    });
    let kind_fixed = match name {
        "main" => "the game".to_string(),
        "sdk" => "nnSdk".to_string(),
        "rtld" => "rtld".to_string(),
        _ => kind(&comps, &exports),
    };
    Ok(Scanned {
        name: name.to_string(),
        m: ModFacts {
            bid,
            sizes,
            bss,
            far: end > 0x7000000,
            imports,
            exports,
            comps,
            kind: kind_fixed,
        },
        sdkver,
        ounce,
        comps: mw,
        marker_hits,
        main,
    })
}

pub fn facts_of(exefs: &Path, romfs: &Path) -> Result<Facts> {
    let mut paths: Vec<_> = std::fs::read_dir(exefs)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|e| e.path())
        .collect();
    paths.sort();
    let mut nsos = Vec::new();
    for p in paths {
        if !p.is_file() {
            continue;
        }
        let mut magic = [0u8; 4];
        let ok = File::open(&p)?.read(&mut magic)? == 4 && &magic == b"NSO0";
        if ok {
            nsos.push(p);
        }
    }
    let scanned: Vec<Result<Scanned>> = nsos
        .par_iter()
        .map(|p| scan_module(p, &p.file_name().unwrap().to_string_lossy()))
        .collect();
    let mut f = Facts::default();
    for s in scanned {
        let s = s?;
        for v in s.sdkver {
            f.sdkver.insert(v);
        }
        if s.name == "sdk" {
            f.ounce = s.ounce;
        }
        for c in s.comps {
            f.comps.insert(c);
        }
        for n in &s.marker_hits {
            f.markers
                .entry((*n).to_string())
                .or_default()
                .push(s.name.clone());
        }
        if let Some(m) = s.main {
            f.unity = m.unity.into_iter().collect();
            f.glslc = m.glslc;
            f.glsl_src = m.glsl_src;
            f.gl_names = m.gl_names;
            f.gl_src = m.gl_src;
            f.main_markers = s.marker_hits.iter().map(|n| n.to_string()).collect();
        }
        f.mods.push((s.name, s.m));
    }
    let (sysres, program) = npdm(&exefs.join("main.npdm"))?;
    f.sysres = sysres;
    f.program = program;
    let major: BTreeSet<String> = f
        .sdkver
        .iter()
        .map(|v| v.split('_').next().unwrap_or("").to_string())
        .collect();
    f.sdk = major
        .iter()
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|v| v.parse::<u64>().ok())
        .min()
        .unwrap_or(0);

    // RomFS: counts by top level (two levels: Unity keeps everything under Data/), extension and header magic
    let r = &mut f.romfs;
    r.biggest = (0, String::new());
    romfs_for_each(romfs, &mut |path, size, h| {
        r.n += 1;
        r.bytes += size;
        let stripped = path.trim_matches('/');
        let parts: Vec<&str> = stripped.split('/').collect();
        let top = if parts.len() > 2 {
            parts[..2].join("/")
        } else if parts.len() > 1 {
            parts[0].to_string()
        } else {
            "(root)".into()
        };
        r.tops.add(&top, 1);
        r.topb.add(&top, size);
        let ext = splitext(path).to_lowercase();
        r.exts.add(if ext.is_empty() { "(none)" } else { &ext }, 1);
        r.extb.add(&ext, size);
        if parts.len() > 1 {
            r.dirs.add(parts[0], 1);
        } else {
            r.root.insert(parts[0].to_string());
        }
        let cand = (size, stripped.to_string());
        if cand > r.biggest {
            r.biggest = cand;
        }
        let mag = MAGICS
            .iter()
            .find(|(m, _)| h.starts_with(m))
            .map(|(_, n)| *n);
        if let Some(mag) = mag {
            r.mags.add(mag, 1);
            r.magb.add(mag, size);
            if PACKED.contains(&mag) {
                r.extc.add(&ext, 1);
            }
        }
    })?;
    Ok(f)
}

/// `os.path.splitext(path)[1]` (a leading dot of the basename is not an extension).
pub fn splitext(path: &str) -> &str {
    let base_start = path.rfind('/').map_or(0, |i| i + 1);
    let base = &path[base_start..];
    let lead = base.len() - base.trim_start_matches('.').len();
    match base[lead..].rfind('.') {
        Some(i) => &base[lead + i..],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ext() {
        assert_eq!(splitext("/a/b.tar.gz"), ".gz");
        assert_eq!(splitext("/a/.hidden"), "");
        assert_eq!(splitext("/a.b/c"), "");
        assert_eq!(splitext("/a/..x.y"), ".y");
        assert_eq!(splitext("/a/b."), ".");
    }
}
