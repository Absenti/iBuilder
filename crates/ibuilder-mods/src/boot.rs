//! The boot's hook: RomFS files and cheats. Read at each boot, so changing them needs no rebuild.

use crate::chain::Log;
use crate::error::{ModsError, Result};
use crate::pyfmt::{self, os_error};
use crate::store::{enabled_mods, files, game_dir, mods_dir, read_state};
use crate::{CHEATS_DIR, STATE_NAME};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The environment `apply_boot` writes to (`os.environ`, or the dict a test passes).
pub trait Environ {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&mut self, name: &str, value: &str);
}

impl Environ for BTreeMap<String, String> {
    fn get(&self, name: &str) -> Option<String> {
        BTreeMap::get(self, name).cloned()
    }
    fn set(&mut self, name: &str, value: &str) {
        self.insert(name.to_string(), value.to_string());
    }
}

/// One line of a cheat file that is a `[section]` header: `^[ \t]*\[([^\]\r\n]+)\][ \t]*$` (re.M; the text has no `\n` inside a segment).
fn section_line(seg: &str) -> Option<(&str, &str, &str)> {
    let rest = seg.trim_start_matches([' ', '\t']);
    let lead = &seg[..seg.len() - rest.len()];
    let inner = rest.strip_prefix('[')?;
    let close = inner.find(']')?;
    let name = &inner[..close];
    if name.is_empty() || name.contains('\r') {
        return None;
    }
    let trail = &inner[close + 1..];
    if trail.chars().all(|c| c == ' ' || c == '\t') {
        Some((lead, name, trail))
    } else {
        None
    }
}

/// `cheat_sections`: the names of a cheat file's `[toggle]` sections in file order (a `{Master}` is always on and not listed). A line of `[`...`]` only; the
/// engine reads the same text.
pub fn cheat_sections(text: &str) -> Vec<String> {
    text.split('\n')
        .filter_map(section_line)
        .map(|(_, n, _)| pyfmt::strip(n).to_string())
        .filter(|n| !n.is_empty())
        .collect()
}

/// `cheat_file_bid`: the first 16 hex digits (upper case) of a cheat file's name, which is its build id, or `None`.
pub fn cheat_file_bid(name: &str) -> Option<String> {
    let stem = pyfmt::splitext_stem(pyfmt::basename(name));
    if stem.chars().count() >= 16 && pyfmt::hex_match(stem) {
        Some(stem.to_uppercase().chars().take(16).collect())
    } else {
        None
    }
}

fn os(e: std::io::Error, p: &Path) -> ModsError {
    ModsError::Os(os_error(&e, p))
}

/// `write_cheats_folder`: `Documents/games/<id>/cheats-run/`: for each build id the enabled mods have a cheat file for, one `<id>.txt` (their files joined; a
/// section name two mods share gets the mod's name added), `<id>.enabled` (the sections the player turned on, one per line: the engine's own state file) and
/// `<id>.origin.tsv` (name in the joined file, mod, name in the mod's file, for the in-game menu to write a toggle back to `mods.tsv`). Returns the folder, or
/// `None` when there is nothing to run.
pub fn write_cheats_folder(docs: &Path, tid: &str, log: Log) -> Result<Option<PathBuf>> {
    let state = read_state(&game_dir(docs, tid).join(STATE_NAME))?;
    let mut by_bid: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for m in enabled_mods(docs, tid)? {
        let base = mods_dir(docs, tid).join(&m).join("cheats");
        let rels = if base.is_dir() { files(&base) } else { vec![] };
        for rel in rels {
            let Some(bid) = cheat_file_bid(&rel) else {
                log(&format!(
                    "mods: {m}/cheats/{rel} is not named by a build id: the engine cannot pick it"
                ));
                continue;
            };
            let path = base.join(&rel);
            let raw = fs::read(&path).map_err(|e| os(e, &path))?;
            let text = pyfmt::universal_newlines(&String::from_utf8_lossy(&raw));
            match by_bid.iter_mut().find(|(b, _)| *b == bid) {
                Some((_, parts)) => parts.push((m.clone(), text)),
                None => by_bid.push((bid, vec![(m.clone(), text)])),
            }
        }
    }
    let out = game_dir(docs, tid).join(CHEATS_DIR);
    let _ = fs::remove_dir_all(&out);
    if by_bid.is_empty() {
        return Ok(None);
    }
    fs::create_dir(&out).map_err(|e| os(e, &out))?;
    for (bid, parts) in &by_bid {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for (_m, text) in parts {
            let mut seen: Vec<String> = cheat_sections(text);
            seen.sort();
            seen.dedup();
            for n in seen {
                *counts.entry(n).or_insert(0) += 1;
            }
        }
        let (mut body, mut on, mut origin) = (Vec::new(), Vec::new(), Vec::new());
        for (m, text) in parts {
            let lines: Vec<String> = text
                .split('\n')
                .map(|seg| match section_line(seg) {
                    Some((lead, name, trail)) => {
                        let n = pyfmt::strip(name);
                        let new = if counts.get(n).copied().unwrap_or(0) > 1 {
                            format!("{n} ({m})")
                        } else {
                            n.to_string()
                        };
                        origin.push(format!("{new}\t{m}\t{n}"));
                        if state
                            .cheat
                            .get(&(m.clone(), n.to_string()))
                            .copied()
                            .unwrap_or(false)
                        {
                            on.push(new.clone());
                        }
                        format!("{lead}[{new}]{trail}")
                    }
                    None => seg.to_string(),
                })
                .collect();
            body.push(lines.join("\n"));
        }
        let w = |ext: &str, text: String| {
            fs::write(out.join(format!("{bid}.{ext}")), text)
                .map_err(|e| os(e, &out.join(format!("{bid}.{ext}"))))
        };
        w("txt", body.join("\n") + "\n")?;
        w("enabled", on.iter().map(|n| format!("{n}\n")).collect())?;
        w(
            "origin.tsv",
            origin.iter().map(|r| format!("{r}\n")).collect(),
        )?;
    }
    Ok(Some(out))
}

/// `romfs_overlays`: the enabled mods' `romfs/` folders, the first one winning a file two mods both have (the engine tries them in order).
pub fn romfs_overlays(docs: &Path, tid: &str) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for m in enabled_mods(docs, tid)? {
        let d = mods_dir(docs, tid).join(&m).join("romfs");
        if d.is_dir() && !d.to_string_lossy().contains(':') {
            out.push(d); // the engine splits the list on ':'
        }
    }
    Ok(out)
}

/// `apply_boot`: set the engine's two switches for the enabled mods before the boot: `DM_ROMFS_OVERLAY` and `DM_CHEATS`. A switch the environment already
/// names wins (a harness, `Documents/env.txt`). Returns `{name: value}` set. An `OSError` is said and the game starts without the rest; anything else the
/// Python lets through ([`ModsError::Unhandled`]) is returned to the caller (the pipeline's `apply_mods` logs it).
pub fn apply_boot(
    docs: &Path,
    tid: &str,
    log: Log,
    env: &mut dyn Environ,
) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    if tid.is_empty() {
        return Ok(out);
    }
    let mut run = |out: &mut Vec<(String, String)>| -> Result<()> {
        let dirs = romfs_overlays(docs, tid)?;
        if !dirs.is_empty() && env.get("DM_ROMFS_OVERLAY").is_none() {
            let v = dirs
                .iter()
                .map(|d| d.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(":");
            env.set("DM_ROMFS_OVERLAY", &v);
            out.push(("DM_ROMFS_OVERLAY".into(), v));
            let n: usize = dirs.iter().map(|d| files(d).len()).sum();
            log(&format!(
                "mods: RomFS overlay: {n} file(s) from {} mod(s) read before the game's own",
                dirs.len()
            ));
        }
        if let Some(folder) = write_cheats_folder(docs, tid, log)?
            && env.get("DM_CHEATS").is_none()
        {
            let v = folder.to_string_lossy().into_owned();
            env.set("DM_CHEATS", &v);
            out.push(("DM_CHEATS".into(), v.clone()));
            log(&format!(
                "mods: cheats: {v} (the engine picks the file by the game's build id)"
            ));
        }
        Ok(())
    };
    match run(&mut out) {
        Ok(()) => Ok(out),
        Err(ModsError::Os(e)) => {
            log(&format!(
                "mods: not applied at the boot ({e}) -- the game starts without them"
            ));
            Ok(out)
        }
        Err(e) => Err(e),
    }
}
