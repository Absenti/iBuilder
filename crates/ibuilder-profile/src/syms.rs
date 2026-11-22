//! `dm/tools/nso_syms.py`: the dynamic symbols of an NSO image, the `nn::` area of a symbol, and the host's HLE
//! hook names (read from the engine's C sources).
use crate::error::{Error, Result, fmt_err};
use crate::pyutil::is_py_space;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// One dynamic symbol: `(name, defined, bind)`. Names are decoded as latin-1 like the Python.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sym {
    pub name: String,
    pub defined: bool,
    pub bind: u8,
}

fn rd<const N: usize>(img: &[u8], off: usize) -> Result<[u8; N]> {
    off.checked_add(N)
        .and_then(|e| img.get(off..e))
        .map(|b| b.try_into().unwrap())
        .ok_or_else(|| {
            Error::Format(format!(
                "unpack_from requires a buffer of at least {} bytes",
                off.saturating_add(N)
            ))
        })
}

/// `dynsyms(img)`: reads `MOD0`, the dynamic table (first of each tag wins), then the symbol table; symbol 0 and
/// empty names are skipped.
pub fn dynsyms(img: &[u8]) -> Result<Vec<Sym>> {
    let mod0 = u32::from_le_bytes(rd(img, 4)?) as usize;
    if img.get(mod0..mod0 + 4) != Some(b"MOD0") {
        return fmt_err("no MOD0");
    }
    let rel = i32::from_le_bytes(rd(img, mod0 + 4)?) as i64;
    let dyn_off = usize::try_from(mod0 as i64 + rel)
        .map_err(|_| Error::Format("negative dynamic table offset".into()))?;
    let mut tags: Vec<(i64, u64)> = Vec::new();
    for i in 0..4096usize {
        let t = i64::from_le_bytes(rd(img, dyn_off.saturating_add(i * 16))?);
        let v = u64::from_le_bytes(rd(img, dyn_off.saturating_add(i * 16 + 8))?);
        if t == 0 {
            break;
        }
        if !tags.iter().any(|(k, _)| *k == t) {
            tags.push((t, v));
        }
    }
    let tag = |t: i64| tags.iter().find(|(k, _)| *k == t).map(|(_, v)| *v);
    let (Some(symtab), Some(strtab)) = (tag(6), tag(5)) else {
        return fmt_err("KeyError: the dynamic table has no DT_SYMTAB / DT_STRTAB");
    };
    let n: i64 = match tag(4) {
        Some(hash) => u32::from_le_bytes(rd(img, hash as usize + 4)?) as i64,
        None => (strtab as i64 - symtab as i64).div_euclid(24),
    };
    let mut out = Vec::new();
    for i in 1..n.max(0) as usize {
        let base = (symtab as usize).saturating_add(i * 24);
        let name = u32::from_le_bytes(rd(img, base)?) as usize;
        let info = rd::<1>(img, base + 4)?[0];
        let shndx = u16::from_le_bytes(rd(img, base + 6)?);
        let s0 = (strtab as usize).saturating_add(name);
        let tail = img
            .get(s0..)
            .ok_or_else(|| Error::Format("subsection not found".into()))?;
        let e = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::Format("subsection not found".into()))?;
        if e > 0 {
            out.push(Sym {
                name: tail[..e].iter().map(|&b| b as char).collect(),
                defined: shndx != 0,
                bind: info >> 4,
            });
        }
    }
    Ok(out)
}

/// `area(s)`: `nn::<name>` for `_ZN2nn<len><name>...`, `nvn`, `other C++` for `_Z...`, else `C / libc`.
pub fn area(s: &str) -> String {
    let rest = s
        .strip_prefix("_ZNK2nn")
        .or_else(|| s.strip_prefix("_ZN2nn"));
    if let Some(r) = rest {
        let digits: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            // regex `(\d+)(\w+)`: \w+ is greedy but backtracks, so the digit run may give back digits: keep Python's
            // behaviour (first attempt: all digits, then at least one \w char must follow)
            let after = &r[digits.len()..];
            let wlen = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .count();
            if wlen > 0 {
                let n: usize = digits.parse().unwrap_or(0);
                let word: String = after
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                return format!("nn::{}", word.chars().take(n).collect::<String>());
            }
            // backtrack: give the last digit to \w+ (only matters for names like `_ZN2nn5`)
            if digits.len() > 1 {
                let n: usize = digits[..digits.len() - 1].parse().unwrap_or(0);
                let word = format!(
                    "{}{}",
                    &digits[digits.len() - 1..],
                    after
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect::<String>()
                );
                return format!("nn::{}", word.chars().take(n).collect::<String>());
            }
        }
    }
    if s.starts_with("nvn") {
        "nvn".to_string()
    } else if s.starts_with("_Z") {
        "other C++".to_string()
    } else {
        "C / libc".to_string()
    }
}

/// The names the host resolves imports to (`host_hooks`): HLE / HLE_FORCE hooks in `dm/host/*.c` and the gfx
/// sources (nn::vi, nv::, NVN), the libc passthrough table (`hle_table.c` `L(...)`), the Vulkan layer (`VK`/`VKB`
/// in `dm/gfx/vk/*.m`).
#[derive(Clone, Debug, Default)]
pub struct HookSet(pub BTreeSet<String>);

impl HookSet {
    pub fn contains(&self, s: &str) -> bool {
        self.0.contains(s)
    }
    pub fn from_names<I: IntoIterator<Item = String>>(names: I) -> HookSet {
        HookSet(names.into_iter().collect())
    }

    /// `host_hooks(host, forced)`: `host` is the engine's `dm/host` directory; `forced = false` leaves out the
    /// `HLE_FORCE` hooks (some wrap the module's own function, so the module that defines it is still needed).
    pub fn from_host_dir(host: &Path, forced: bool) -> Result<HookSet> {
        let mut hooks = BTreeSet::new();
        let gfx = host.join("../gfx");
        let mut cfiles = glob_ext(host, "c");
        cfiles.extend(glob_ext(&gfx, "c"));
        cfiles.extend(glob_ext(&gfx.join("nvn"), "c"));
        for f in cfiles {
            let t = String::from_utf8_lossy(&fs::read(&f)?).into_owned();
            hle_names(&t, "HLE(", &mut hooks);
            if forced {
                for p in ["HLE_FORCE(", "HLE_FORCE_DIRECT(", "HLE_FORCE_ALIAS("] {
                    hle_names(&t, p, &mut hooks);
                }
            }
            if f.file_name().is_some_and(|n| n == "hle_table.c") {
                l_names(&t, &mut hooks);
            }
        }
        for f in glob_ext(&gfx.join("vk"), "m") {
            let t = String::from_utf8_lossy(&fs::read(&f)?).into_owned();
            vk_names(&t, &mut hooks);
        }
        Ok(HookSet(hooks))
    }

    /// One name per line (`#` comments), the form `ibuilder-profile hooks` writes for a bundle without the sources.
    pub fn parse_list(text: &str) -> HookSet {
        HookSet(
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_string)
                .collect(),
        )
    }

    pub fn to_list(&self) -> String {
        let mut s = String::from("# HLE hook names (dm/host + dm/gfx sources), one per line\n");
        for h in &self.0 {
            s.push_str(h);
            s.push('\n');
        }
        s
    }
}

fn glob_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == ext) && p.is_file())
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `\b<prefix>\s*"([^"]+)"` with the prefix `NAME(`
fn hle_names(t: &str, prefix: &str, out: &mut BTreeSet<String>) {
    let mut from = 0;
    while let Some(p) = t[from..].find(prefix) {
        let s = from + p;
        from = s + prefix.len();
        if t[..s].chars().next_back().is_some_and(is_word) {
            continue; // \b before the first (word) char of the prefix
        }
        let rest = t[s + prefix.len()..].trim_start_matches(is_py_space);
        if let Some(r) = rest.strip_prefix('"')
            && let Some(e) = r.find('"')
            && e > 0
        {
            out.insert(r[..e].to_string());
        }
    }
}

/// `\bL\((\w+)\)`
fn l_names(t: &str, out: &mut BTreeSet<String>) {
    let mut from = 0;
    while let Some(p) = t[from..].find("L(") {
        let s = from + p;
        from = s + 2;
        if t[..s].chars().next_back().is_some_and(is_word) {
            continue;
        }
        let r = &t[s + 2..];
        let w: String = r.chars().take_while(|c| is_word(*c)).collect();
        if !w.is_empty() && r[w.len()..].starts_with(')') {
            out.insert(w);
        }
    }
}

/// `^VKB?\((vk\w+),` (multi-line)
fn vk_names(t: &str, out: &mut BTreeSet<String>) {
    for line in t.split('\n') {
        let r = if let Some(r) = line.strip_prefix("VKB(") {
            r
        } else if let Some(r) = line.strip_prefix("VK(") {
            r
        } else {
            continue;
        };
        if let Some(rest) = r.strip_prefix("vk") {
            let w: String = rest.chars().take_while(|c| is_word(*c)).collect();
            if !w.is_empty() && rest[w.len()..].starts_with(',') {
                out.insert(format!("vk{w}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn areas() {
        assert_eq!(area("_ZN2nn2os17SetMemoryHeapSizeEm"), "nn::os");
        assert_eq!(
            area("_ZNK2nn3mem17StandardAllocator10InitializeEPvmb"),
            "nn::mem"
        );
        assert_eq!(area("nvnDeviceInitialize"), "nvn");
        assert_eq!(area("_ZNSt3__16vectorE"), "other C++");
        assert_eq!(area("memcpy"), "C / libc");
    }
    #[test]
    fn hook_scanners() {
        let mut h = BTreeSet::new();
        hle_names(
            "HLE( \"nn::a\" , f);\nXHLE(\"x\") HLE_FORCE(\"force\") HLE(\n\"b\")",
            "HLE(",
            &mut h,
        );
        assert_eq!(h.iter().cloned().collect::<Vec<_>>(), vec!["b", "nn::a"]);
        let mut f = BTreeSet::new();
        hle_names("HLE_FORCE(\"force\") HLE(\"b\")", "HLE_FORCE(", &mut f);
        assert_eq!(f.len(), 1);
        let mut l = BTreeSet::new();
        l_names("L(memcpy) XL(no) L(strlen)\nL( bad)", &mut l);
        assert_eq!(
            l.iter().cloned().collect::<Vec<_>>(),
            vec!["memcpy", "strlen"]
        );
        let mut v = BTreeSet::new();
        vk_names(
            "VK(vkCreateInstance, a)\n VK(vkNo, b)\nVKB(vkCmdDraw, c)\nVK(vkBad)",
            &mut v,
        );
        assert_eq!(
            v.iter().cloned().collect::<Vec<_>>(),
            vec!["vkCmdDraw", "vkCreateInstance"]
        );
    }
}
