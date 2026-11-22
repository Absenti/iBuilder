//! Shaders the running game met that its library lacks (`shader_device.live_package` + `_live_translate`, paches/0008): the host hands over each one's control section and code area,
//! this makes a one-shader-package of it in the split's record format and runs a partial build over it with the library's tables.
//!
//! Package S (`ibuilder-shadersplit`) owns `shader_split.py`; `ids` below is the small part of it live translation needs (`skey`, `glslc_stage`, `nvn2_ctl`, the stage names), **copied**
//! here (about 70 lines) because the crate did not exist when this was written; once it does, `ids` should become a re-export of it.
use crate::build::{BuildEnv, BuildOptions, build};
use crate::cache::write_atomic;
use crate::finish::Finisher;
use crate::game::Game;
use crate::pyjson::{Opts, dumps};
use ibuilder_shader::romfs::Image;
use ibuilder_shader::tools::ShaderTools;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

/// the parts of `shader_split.py` the live path calls
pub mod ids {
    use sha2::{Digest, Sha256};

    pub const CTL_MAGIC: [u8; 4] = [0x34, 0x12, 0x76, 0x98];
    pub const GFX_MAGIC: [u8; 4] = [0x78, 0x56, 0x34, 0x12];
    pub const CS_MAGIC: [u8; 4] = [0x79, 0x56, 0x34, 0x12];
    pub const NVN2_KEY_CTL: usize = 0x48;

    /// `STAGE_NAME`: the stage names of the map's third column
    pub fn stage_name(st: &str) -> Option<&'static str> {
        Some(match st {
            "vs" => "vertex",
            "hs" => "hull",
            "ds" => "domain",
            "gs" => "geometry",
            "ps" => "pixel",
            "cs" => "compute",
            "ts" => "task",
            "ms" => "mesh",
            "rt" => "raytracing",
            _ => return None,
        })
    }

    fn u32_at(b: &[u8], o: usize) -> Option<u32> {
        b.get(o..o + 4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    }

    /// `nvn2_ctl`: an NVN2 control section: the magic and version 0xc9 at +4
    pub fn nvn2_ctl(ctl: &[u8]) -> bool {
        ctl.len() >= NVN2_KEY_CTL && ctl[..4] == CTL_MAGIC && ctl[4..8] == [0xc9, 0, 0, 0]
    }

    /// `ctl_len`: the control bytes the key hashes
    pub fn ctl_len(ctl: &[u8]) -> usize {
        if nvn2_ctl(ctl) {
            return NVN2_KEY_CTL;
        }
        let n = u32_at(ctl, 0x20).unwrap_or(0) as usize;
        if (0x760..0x880).contains(&n) {
            n
        } else {
            0x880
        }
    }

    /// `key_ctl` with the profile's `shader_key_skip` range (`(offset, length)`) as zeros
    pub fn key_ctl(ctl: &[u8], skip: (usize, usize)) -> Vec<u8> {
        let c = &ctl[..ctl_len(ctl).min(ctl.len())];
        if nvn2_ctl(ctl) {
            let mut v = c.to_vec();
            v[0x30..0x34].fill(0);
            return v;
        }
        let (o, l) = skip;
        if l != 0 && o + l <= c.len() {
            let mut v = c.to_vec();
            v[o..o + l].fill(0);
            v
        } else {
            c.to_vec()
        }
    }

    /// `skey(ctl, code)`: the shader's key (16 hex of the sha256 over the key control bytes and the code)
    pub fn skey(ctl: &[u8], code: &[u8], skip: (usize, usize)) -> String {
        let mut h = Sha256::new();
        h.update(key_ctl(ctl, skip));
        h.update(code);
        h.finalize()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// `sph_stage`: a graphics code area's stage from its SPH word 0
    pub fn sph_stage(code: &[u8]) -> Option<&'static str> {
        let w = u32_at(code, 0x30)?;
        if (w >> 10) & 0xf == 0 && w & 0x1f == 1 {
            return Some("vs");
        }
        match (w >> 10) & 0xf {
            1 => Some("vs"),
            2 => Some("hs"),
            3 => Some("ds"),
            4 => Some("gs"),
            5 => Some("ps"),
            _ => None,
        }
    }

    /// `nvn2_stage`: compute, task, mesh and ray tracing by the control section's stage byte (+0x56: 5, 6, 7, 8), else the SPH at the code area's start
    pub fn nvn2_stage(ctl: &[u8], code: &[u8]) -> Option<&'static str> {
        if ctl.len() > 0x56 && matches!(ctl[0x56], 5..=8) {
            return Some(match ctl[0x56] {
                5 => "cs",
                6 => "ts",
                7 => "ms",
                _ => "rt",
            });
        }
        if code.len() >= 4 {
            let mut v = vec![0u8; 0x30];
            v.extend_from_slice(&code[..4]);
            sph_stage(&v)
        } else {
            None
        }
    }

    /// `glslc_stage`: a GLSLC output's stage, from its code area's header, or compute when the code has no header and the control's thread group (+0x748) is filled
    pub fn glslc_stage(ctl: &[u8], code: &[u8]) -> Option<&'static str> {
        if nvn2_ctl(ctl) {
            return nvn2_stage(ctl, code);
        }
        let head: &[u8] = &code[..code.len().min(4)];
        let headerless_cs = head != CS_MAGIC
            && head != GFX_MAGIC
            && ctl.len() >= 0x754
            && (0..3).all(|i| u32_at(ctl, 0x748 + 4 * i).unwrap_or(0) != 0);
        if head == CS_MAGIC || headerless_cs {
            Some("cs")
        } else if head == GFX_MAGIC && code.len() >= 0x34 {
            sph_stage(code)
        } else {
            None
        }
    }
}

pub struct LiveEntry {
    pub key: String,
    pub ctl: Vec<u8>,
    pub code: Vec<u8>,
}

#[derive(Default, Debug)]
pub struct LiveOut {
    /// key -> the stage it was translated for (`vertex`, `pixel`, ...)
    pub ok: BTreeMap<String, String>,
    /// key -> why it was refused
    pub bad: BTreeMap<String, String>,
    /// the files the host reads, by name (`<key>_b.json`, `<key>_b.metal`, a cut unit's others)
    pub files: BTreeMap<String, Vec<u8>>,
}

/// Builds the finisher for the folder holding `<game>/game.tsv` (the live build's merged profile): its shader map holds the live rows
pub type FinisherFactory<'a> = &'a dyn Fn(&Path, &str) -> Result<Box<dyn Finisher + 'a>, String>;

type StrMap = BTreeMap<String, String>;

/// `live_package`: the package and the table rows of the shaders the host met. Each is checked as the split checks what it reads: its key is the host's (`skey` over the profile's key rules),
/// its stage is known (`glslc_stage`), the record cannot be cut wrongly. Writes `<pkg>/<st>_live.bin`, `<tables>/live_map.tsv` and `live_facts.tsv`.
pub fn live_package(
    entries: &[LiveEntry],
    game: &Game,
    pkg_dir: &Path,
    tables_dir: &Path,
) -> Result<(StrMap, StrMap), String> {
    std::fs::create_dir_all(pkg_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(tables_dir).map_err(|e| e.to_string())?;
    let skip = game.key_skip();
    let mut ok = BTreeMap::new();
    let mut bad = BTreeMap::new();
    let mut recs: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
    let mut rows: Vec<(String, usize, String, String)> = Vec::new();
    let mut facts: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for LiveEntry { key, ctl, code } in entries {
        let one = || -> Result<(&'static str, Vec<u8>, Option<serde_json::Value>), String> {
            if ctl.len() < 0x80 || code.len() < 0x10 {
                return Err(format!(
                    "control section of {} bytes, code area of {}",
                    ctl.len(),
                    code.len()
                ));
            }
            let sk = ids::skey(ctl, code, skip);
            if &sk != key {
                return Err(format!("its bytes give key {sk}, not this one"));
            }
            let st = ids::glslc_stage(ctl, code)
                .ok_or_else(|| "stage not recognised (None)".to_string())?;
            let kb = crate::records::unhex(key)
                .filter(|b| b.len() == 8)
                .ok_or_else(|| format!("non-hexadecimal number found in fromhex() arg: {key:?}"))?;
            let mut rec = Vec::with_capacity(0x38 + code.len() + ctl.len());
            rec.extend_from_slice(b"BTCDBTCD");
            rec.extend_from_slice(&kb);
            rec.extend_from_slice(&[0u8; 0x28]);
            rec[0x28..0x2c].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
            rec[0x2c..0x30].copy_from_slice(&(code.len() as u32).to_le_bytes());
            rec.extend_from_slice(code);
            rec.extend_from_slice(ctl);
            if memchr::memmem::find(&rec[0x38..], &ids::CTL_MAGIC).map(|i| i + 0x38)
                != Some(0x38 + code.len())
            {
                return Err("the control magic appears in the code area".into());
            }
            if memchr::memmem::find_iter(&rec, b"BTCDBTCD").count() != 1 {
                return Err("the record separator appears inside the record".into());
            }
            let mut fact = None;
            if st == "cs" {
                let at = if ids::nvn2_ctl(ctl) {
                    (0x58usize, 0x64usize)
                } else {
                    (0x748, 0x754)
                };
                let u = |o: usize| {
                    ctl.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| format!("unpack_from requires a buffer of at least {} bytes (actual buffer size is {})", o + 4, ctl.len()))
                };
                let (x, y, z) = (u(at.0)?, u(at.0 + 4)?, u(at.0 + 8)?);
                let sh = u(at.1)?;
                if (x as u128) * (y as u128) * (z as u128) == 0 {
                    return Err("compute shader with a thread group of 0".into());
                }
                fact = Some(json!({"local": [x, y, z], "shared": (sh as u64).div_ceil(16) * 16}));
            }
            Ok((st, rec, fact))
        };
        match one() {
            Err(e) => {
                bad.insert(key.clone(), e);
            }
            Ok((st, rec, fact)) => {
                if let Some(f) = fact {
                    facts.insert(key.clone(), f);
                }
                let name = format!("{st}_live.bin");
                let v = recs.entry(name.clone()).or_default();
                v.push(rec);
                rows.push((
                    name,
                    v.len() - 1,
                    ids::stage_name(st).unwrap().to_string(),
                    key.clone(),
                ));
                ok.insert(key.clone(), ids::stage_name(st).unwrap().to_string());
            }
        }
    }
    for (name, rr) in &recs {
        write_atomic(&pkg_dir.join(name), &rr.concat()).map_err(|e| e.to_string())?;
    }
    let mut map = String::new();
    for (n, i, s, k) in &rows {
        map.push_str(&format!("{n}\t{i}\t{s}\t{k}\tlive\n"));
    }
    write_atomic(&tables_dir.join("live_map.tsv"), map.as_bytes()).map_err(|e| e.to_string())?;
    let mut ft = String::new();
    for (k, f) in &facts {
        ft.push_str(&format!("{k}\t{}\n", dumps(f, &Opts::COMPACT_SORTED)));
    }
    write_atomic(&tables_dir.join("live_facts.tsv"), ft.as_bytes()).map_err(|e| e.to_string())?;
    Ok((ok, bad))
}

/// `live_translate`: translates shaders the game used in play that its library lacks, with the tables of the game's library (`tables_dir`: `shader_map.tsv` and the others the build reads) and its
/// profile table (`game_table_dir`: `game.tsv`): a partial build of just those keys. `work` is wiped first. `cas` is the shared cache folder, if there is one.
#[allow(clippy::too_many_arguments)]
pub fn live_translate(
    entries: &[LiveEntry],
    tables_dir: &Path,
    game_table_dir: &Path,
    game_id: &str,
    work: &Path,
    chain: &str,
    tools: &dyn ShaderTools,
    make_finisher: FinisherFactory,
    log: &(dyn Fn(&str) + Sync),
    cas: Option<&Path>,
) -> Result<LiveOut, String> {
    let _ = std::fs::remove_dir_all(work);
    let (pkg, tab, prof) = (work.join("pkg"), work.join("tables"), work.join("profile"));
    let gdir = prof.join(game_id);
    std::fs::create_dir_all(&gdir).map_err(|e| e.to_string())?;
    // the library's tables over the profile's own (as build_shaders_on_device lays a build's profile out)
    for d in [game_table_dir, tables_dir] {
        let rd = std::fs::read_dir(d).map_err(|e| format!("{}: {e}", d.display()))?;
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if e.path().is_file() && (n.ends_with(".tsv") || n == "game.tsv") {
                std::fs::copy(e.path(), gdir.join(&n)).map_err(|e| format!("{n}: {e}"))?;
            }
        }
    }
    let game = Game::load(&gdir)?;
    let (ok, bad) = live_package(entries, &game, &pkg, &tab)?;
    let mut out = LiveOut {
        ok,
        bad,
        files: BTreeMap::new(),
    };
    if out.ok.is_empty() {
        return Ok(out);
    }
    // the live rows replace any of the same keys (they name packages this build does not have)
    let smap = gdir.join("shader_map.tsv");
    let keep: Vec<String> = match std::fs::read_to_string(&smap) {
        Ok(t) => t
            .split_inclusive('\n')
            .filter(|ln| !{
                let c: Vec<&str> = ln.split('\t').collect();
                c.len() > 3 && out.ok.contains_key(c[3])
            })
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    };
    let mut text = keep.concat();
    text.push_str(&std::fs::read_to_string(tab.join("live_map.tsv")).map_err(|e| e.to_string())?);
    write_atomic(&smap, text.as_bytes()).map_err(|e| e.to_string())?;
    for n in ["shader_pairs.tsv", "pso_pairs.tsv"] {
        // a pair names its partner's record, which is in no package here (the split's are gone on the phone)
        let p = gdir.join(n);
        if let Ok(t) = std::fs::read_to_string(&p) {
            let kept: String = t
                .split_inclusive('\n')
                .filter(|ln| ln.starts_with('#') || !out.ok.keys().any(|k| ln.contains(k.as_str())))
                .collect();
            write_atomic(&p, kept.as_bytes()).map_err(|e| e.to_string())?;
        }
    }
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(gdir.join("shader_facts.tsv"))
            .map_err(|e| e.to_string())?;
        f.write_all(&std::fs::read(tab.join("live_facts.tsv")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    let finisher = make_finisher(&prof, game_id)?;
    let own_cas;
    let cas_dir = match cas {
        Some(c) => c,
        None => {
            own_cas = work.join("out").join("cas");
            std::fs::create_dir_all(&own_cas).map_err(|e| e.to_string())?;
            &own_cas
        }
    };
    let mut o = BuildOptions::new(&pkg, &prof, game_id, &work.join("out"), chain, cas_dir);
    o.keys = Some(out.ok.keys().cloned().collect());
    o.jobs = out.ok.len().clamp(1, 4);
    o.tables = false;
    o.pulling = false;
    o.follow_stop = false; // the game is running: a Pause or Cancel of the chain that came late (after its last check) is not for the shaders the game is waiting on
    let env = BuildEnv {
        tools,
        finisher: &*finisher,
        log,
        cancel: None,
    };
    let rep = build(&o, &env)?;
    let bytes = std::fs::read(&rep.image).map_err(|e| format!("{}: {e}", rep.image.display()))?;
    let img = Image::open(&bytes)?;
    for f in &img.files {
        if out.ok.keys().any(|k| f.name.starts_with(k.as_str())) {
            out.files.insert(f.name.clone(), img.bytes(f).to_vec());
        }
    }
    let gone: Vec<String> = out
        .ok
        .keys()
        .filter(|k| !out.files.contains_key(&format!("{k}_b.json")))
        .cloned()
        .collect();
    for k in gone {
        out.bad
            .insert(k.clone(), "the build wrote no library for it".into());
        out.ok.remove(&k);
    }
    let _ = Sha256::new();
    Ok(out)
}
