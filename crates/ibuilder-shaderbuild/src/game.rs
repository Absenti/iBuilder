//! The game profile folder the build reads (`dm/games/<id>/`: `game.tsv` and the shader tables) and what `dmsh_build.py` / `dmsh_chain.py` derive from it:
//! the shader map (`loc`, `MAP_ROWS`, `MAP_CONFLICT_ROWS`), the answer table (`dmsh_facts.load`), the pair tables (`load_pairs`), `bf_want` / `dual_want`,
//! `profile_rows` and the digest of the profile's inputs (`dmsh_chain.inputs`) that stamps a finished library (`inputs.tsv`).
//!
//! Text decoding: the Python reads these files as UTF-8 text with universal newlines (`splitlines`); the tables are ASCII (keys, hex, JSON written with
//! `ensure_ascii=False` for strings the game gives, in `shader_facts.tsv`) so `str::lines` (`\n` and `\r\n`) is the same for every file the split writes.
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

pub const STAMP: &str = "inputs.tsv";
/// `dmsh_chain.INPUT_TABLES`: the profile's tables the library's digest covers, in this order
pub const INPUT_TABLES: [&str; 8] = [
    "shader_facts.tsv",
    "shader_map.tsv",
    "shader_map.tsv.conflicts.tsv",
    "shader_pairs.tsv",
    "pso_pairs.tsv",
    "bf_want.tsv",
    "geo_mesh.tsv",
    "dual_want.tsv",
];

pub fn io_err(what: &str, p: &Path, e: std::io::Error) -> String {
    format!("{what} {}: {e}", p.display())
}

fn read_text(p: &Path) -> Result<String, String> {
    let b = std::fs::read(p).map_err(|e| io_err("read", p, e))?;
    String::from_utf8(b).map_err(|e| format!("{}: not UTF-8 ({e})", p.display()))
}

/// `game.Game`: `game.tsv`'s rows `(key, values)`, and the folder the tables are in.
#[derive(Clone, Debug)]
pub struct Game {
    pub dir: PathBuf,
    pub id: String,
    pub rows: Vec<(String, Vec<String>)>,
}

/// `shader_device.build`: `profile_dir` is the game's own folder (it holds `game.tsv`) or the folder that holds `<game_id>/`
pub fn game_dir(profile_dir: &Path, game_id: &str) -> PathBuf {
    if profile_dir.join("game.tsv").is_file() {
        profile_dir.to_path_buf()
    } else {
        profile_dir.join(game_id)
    }
}

impl Game {
    pub fn load(dir: &Path) -> Result<Game, String> {
        let text = read_text(&dir.join("game.tsv"))?;
        let mut rows = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let mut c = line.split('\t');
            let k = c.next().unwrap_or("").trim().to_string();
            rows.push((k, c.map(|v| v.trim().to_string()).collect()));
        }
        let id = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Game {
            dir: dir.to_path_buf(),
            id,
            rows,
        })
    }

    /// `g.get(key, default)`: the first row of the key, its values joined by tabs
    pub fn get(&self, key: &str) -> Option<String> {
        self.rows
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.join("\t"))
    }

    pub fn table(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// `dmsh_chain.profile_rows`: the profile keys that change how a shader is built (`nvn_*`, `shader_*`), as bytes
    pub fn profile_rows(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (k, v) in &self.rows {
            if k.starts_with("nvn_") || k.starts_with("shader_") {
                out.extend_from_slice(format!("{k}\t{}\n", v.join("\t")).as_bytes());
            }
        }
        out
    }

    /// `shader_split.KEY_SKIP`: `shader_key_skip` = `<offset>+<length>` (default `0+0`), numbers as `int(x, 0)`
    pub fn key_skip(&self) -> (usize, usize) {
        let v = self.get("shader_key_skip").unwrap_or_else(|| "0+0".into());
        let mut it = v
            .split('+')
            .map(|x| parse_int0(x.trim()).unwrap_or(0) as usize);
        (it.next().unwrap_or(0), it.next().unwrap_or(0))
    }

    /// `dmsh_chain.inputs`: the digest (12 hex) of the profile's inputs to its shader library: the tables (a missing one counts as such) and the `nvn_*` / `shader_*` keys; of the
    /// pipeline record only its `G` rows (`mesh_pairs`), and only when it has any
    pub fn inputs_digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for f in INPUT_TABLES {
            h.update(format!("game/{f}").as_bytes());
            h.update([0u8]);
            match std::fs::read(self.table(f)) {
                Ok(b) => h.update(&b),
                Err(_) => h.update(b"missing"),
            }
            h.update([0u8]);
        }
        let rec = table_pairs(&self.table("pso_manifest.tsv"), Some('G'));
        if !rec.is_empty() {
            h.update(b"game/pso_manifest.tsv G\0");
            // `''.join(f'{v}\t{k}\n' for v, k in sorted(rec))`: the set's (vertex, geometry) tuples in sorted order (a BTreeSet iterates sorted)
            let text: String = rec.iter().map(|(v, k)| format!("{v}\t{k}\n")).collect();
            h.update(text.as_bytes());
            h.update([0u8]);
        }
        h.update(self.profile_rows());
        let d = h.finalize();
        d.iter().take(6).map(|b| format!("{b:02x}")).collect()
    }

    /// `dmsh_chain.mesh_pairs`: the vertex / geometry pairs that get a mesh library (`dmsh_mesh.py`): the pipeline record's `G` rows and `geo_mesh.tsv`
    pub fn mesh_pairs(&self) -> BTreeSet<(String, String)> {
        let mut s = table_pairs(&self.table("pso_manifest.tsv"), Some('G'));
        s.extend(table_pairs(&self.table("geo_mesh.tsv"), None));
        s
    }
}

/// Python's `int(text, 0)`: decimal, `0x`, `0o`, `0b`, with an optional sign and underscores between digits
pub fn parse_int0(t: &str) -> Option<i64> {
    let t = t.trim();
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let t = t.replace('_', "");
    let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i64::from_str_radix(h, 16).ok()?
    } else if let Some(o) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        i64::from_str_radix(o, 8).ok()?
    } else if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        i64::from_str_radix(b, 2).ok()?
    } else {
        t.parse::<i64>().ok()?
    };
    Some(if neg { -v } else { v })
}

/// `dmsh_chain.table_pairs`: `{(vertex key, geometry key)}` of `geo_mesh.tsv` (kind `None`: whitespace separated, `#` comments) or the `G` rows of a pipeline record
/// (kind `Some('G')`: tab separated, first column the row kind); a missing table has none. Keys are cut to 16 characters.
pub fn table_pairs(p: &Path, kind: Option<char>) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let Ok(text) = read_text(p) else { return out };
    for ln in text.lines() {
        let c: Vec<&str> = if let Some(k) = kind {
            let c: Vec<&str> = ln.split('\t').collect();
            if c.first().map(|f| *f == k.to_string()).unwrap_or(false) {
                c[1..].to_vec()
            } else {
                Vec::new()
            }
        } else {
            if ln.trim().is_empty() || ln.starts_with('#') {
                continue;
            }
            ln.split_whitespace().collect()
        };
        if c.len() >= 2 && !c[0].is_empty() && !c[1].is_empty() {
            let cut = |s: &str| s.chars().take(16).collect::<String>();
            out.insert((cut(c[0]), cut(c[1])));
        }
    }
    out
}

/// The shader map: `loc` (key -> (package name, row index)), `pc_of`, the raw rows the digest covers.
#[derive(Default, Debug)]
pub struct ShaderMap {
    /// key -> (package file name, index): first row of a key wins
    pub loc: HashMap<String, (String, i64)>,
    /// key -> the map's `pc` column (column 4), conflicts applied
    pub pc_of: HashMap<String, String>,
    /// key -> the raw map row, newline included (`MAP_ROWS`): what the shader's cache key covers
    pub rows: HashMap<String, Vec<u8>>,
    /// key -> the raw conflict row (`MAP_CONFLICT_ROWS`)
    pub conflict_rows: HashMap<String, Vec<u8>>,
    /// keys of the ray-tracing modules (`RT_KEYS`: stage `rt` / `raytracing`): the Python builds ONE universal library of them (`dmsh_rt_build` / `dmsh_rt_compile`, `rt_library`)
    /// and keeps them out of the per-shader path; this port does not build that library (see `BuildReport::rt_modules`)
    pub rt_keys: std::collections::HashSet<String>,
}

impl ShaderMap {
    pub fn load(map: &Path) -> Result<ShaderMap, String> {
        let raw = std::fs::read(map).map_err(|e| io_err("read", map, e))?;
        let mut conf_path = map.as_os_str().to_owned();
        conf_path.push(".conflicts.tsv");
        let conf_path = PathBuf::from(conf_path);
        // the Python opens the conflicts table unconditionally (a profile without one is a crash there); a missing one is read as an empty one here, which is what an empty table gives it
        let conf = match std::fs::read(&conf_path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io_err("read", &conf_path, e)),
        };
        let mut m = ShaderMap::default();
        for line in raw.split_inclusive(|b| *b == b'\n') {
            let body = line.strip_suffix(b"\n").unwrap_or(line);
            let c: Vec<&[u8]> = body.split(|b| *b == b'\t').collect();
            if c.len() > 4 {
                m.rows
                    .entry(String::from_utf8_lossy(c[3]).into_owned())
                    .or_insert_with(|| line.to_vec());
            }
        }
        for line in conf.split_inclusive(|b| *b == b'\n') {
            let body = line.strip_suffix(b"\n").unwrap_or(line);
            let c: Vec<&[u8]> = body.split(|b| *b == b'\t').collect();
            if c.len() > 1 && !line.starts_with(b"#") && !line.starts_with(b"dm_") {
                m.conflict_rows
                    .entry(String::from_utf8_lossy(c[0]).into_owned())
                    .or_insert_with(|| line.to_vec());
            }
        }
        let text = String::from_utf8_lossy(&raw);
        for ln in text.lines() {
            let c: Vec<&str> = ln.split('\t').collect();
            if c.len() > 4 && c[0].ends_with(".bin") {
                m.pc_of
                    .entry(c[3].to_string())
                    .or_insert_with(|| c[4].to_string());
                if matches!(c[2], "rt" | "raytracing") {
                    m.rt_keys.insert(c[3].to_string());
                }
                if let Ok(i) = c[1].trim().parse::<i64>() {
                    m.loc
                        .entry(c[3].to_string())
                        .or_insert_with(|| (c[0].to_string(), i));
                } else {
                    return Err(format!(
                        "{}: the index column of the row of {} is not a number: {:?}",
                        map.display(),
                        c[3],
                        c[1]
                    ));
                }
            }
        }
        let ctext = String::from_utf8_lossy(&conf);
        for ln in ctext.lines() {
            let c: Vec<&str> = ln.split('\t').collect();
            if c.len() > 1
                && !ln.starts_with('#')
                && !ln.starts_with("dm_")
                && m.pc_of.contains_key(c[0])
            {
                m.pc_of.insert(c[0].to_string(), c[1].to_string());
            }
        }
        Ok(m)
    }
}

/// `dmsh_facts.load`: key -> the answer-table object (key order as in the file: sorted), `{}` when the table does not exist
pub fn load_facts(p: &Path) -> Result<HashMap<String, Value>, String> {
    if !p.exists() {
        return Ok(HashMap::new());
    }
    let text = read_text(p)?;
    let mut out = HashMap::new();
    for l in text.lines() {
        if l.trim().is_empty() || l.starts_with('#') {
            continue;
        }
        let (k, v) = l
            .split_once('\t')
            .ok_or_else(|| format!("{}: a facts row without a tab: {l:.60}", p.display()))?;
        let v: Value =
            serde_json::from_str(v).map_err(|e| format!("{}: facts of {k}: {e}", p.display()))?;
        out.insert(k.to_string(), v);
    }
    Ok(out)
}

/// `dmsh_build.load_pairs`: vertex key -> the pixel shaders it is paired with, both pair tables
pub fn load_pairs(
    game: &Game,
    package_dir: &Path,
) -> Result<HashMap<String, BTreeSet<String>>, String> {
    let mut pairs: HashMap<String, BTreeSet<String>> = HashMap::new();
    for table in ["shader_pairs.tsv", "pso_pairs.tsv"] {
        let mut path = game.table(table);
        if table == "pso_pairs.tsv" && !path.is_file() {
            // the split's copy for profiles predating this table: beside the package folder
            path = package_dir
                .parent()
                .map(|d| d.join(table))
                .unwrap_or_default();
        }
        if table == "pso_pairs.tsv" && !path.is_file() {
            continue;
        }
        let text = read_text(&path)?;
        for ln in text.lines() {
            if ln.trim().is_empty() || ln.starts_with('#') {
                continue;
            }
            let mut w = ln.split_whitespace();
            match (w.next(), w.next(), w.next()) {
                (Some(v), Some(p), None) => {
                    pairs
                        .entry(v.to_string())
                        .or_default()
                        .insert(p.to_string());
                }
                _ => {
                    return Err(format!(
                        "{}: a pair row that is not `<vertex> <pixel>`: {ln:.80}",
                        path.display()
                    ));
                }
            }
        }
    }
    Ok(pairs)
}

/// `BF_WANT` (bf_want.tsv: one key a line) and `DUAL_WANT` (dual_want.tsv: the first word of a line)
pub fn load_key_set(p: &Path, first_word: bool) -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    if let Ok(t) = read_text(p) {
        for line in t.lines() {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            if first_word {
                if let Some(w) = line.split_whitespace().next() {
                    s.insert(w.to_string());
                }
            } else {
                s.insert(line.trim().to_string());
            }
        }
    }
    s
}

/// `dmsh_chain.write_stamp`'s text
pub fn stamp_text(game_id: &str, digest: &str) -> String {
    format!(
        "# the game and the profile inputs this library was built from (dm/gfx/tools/dmsh_chain.py inputs)\ngame\t{game_id}\ninputs\t{digest}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int0() {
        assert_eq!(parse_int0("0x770"), Some(0x770));
        assert_eq!(parse_int0("120"), Some(120));
        assert_eq!(parse_int0("0"), Some(0));
        assert_eq!(parse_int0("0b101"), Some(5));
        assert_eq!(parse_int0("x"), None);
    }

    /// `RT_KEYS`: a row of stage `rt` / `raytracing` is a ray-tracing module (the Python keeps it out of the per-shader path), any other stage is not.
    #[test]
    fn the_ray_tracing_rows_of_the_map_are_told_apart() {
        let dir = std::env::temp_dir().join(format!("ibuilder-rt-map-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let map = dir.join("shader_map.tsv");
        std::fs::write(
            &map,
            "package\tindex\tstage\tdm_ctl_sha16\tsource\n\
             ps_data.bin\t0\tps\t1111111111111111\tA\n\
             rt_g0.bin\t0\traytracing\t2222222222222222\tB\n\
             rt_g0.bin\t1\trt\t3333333333333333\tC\n",
        )
        .unwrap();
        let m = ShaderMap::load(&map).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        let mut rt: Vec<&String> = m.rt_keys.iter().collect();
        rt.sort();
        assert_eq!(rt, ["2222222222222222", "3333333333333333"]);
        assert_eq!(
            m.loc.len(),
            3,
            "every row stays in the map: only the build leaves the RT ones out"
        );
    }
}
