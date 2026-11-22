//! The store and the player's state: `mods.tsv`, the list of mods, the files of a folder, the exefs signature.

use crate::STATE_NAME;
use crate::error::{ModsError, Result};
use crate::pyfmt::{self, os_error};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub fn game_dir(docs: &Path, tid: &str) -> PathBuf {
    docs.join("games").join(tid)
}

pub fn mods_dir(docs: &Path, tid: &str) -> PathBuf {
    game_dir(docs, tid).join("mods")
}

/// `mods.tsv` as `{'mod': {name: on}, 'patch': {(mod, name): on}, 'cheat': {(mod, name): on}}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModState {
    pub mods: BTreeMap<String, bool>,
    pub patch: BTreeMap<(String, String), bool>,
    pub cheat: BTreeMap<(String, String), bool>,
}

/// `read_state(path)`. A missing or unreadable file is an empty state: no mod is on until the player turns it on. A file that is not UTF-8 is a
/// `UnicodeDecodeError` in Python (not an `OSError`), which propagates: [`ModsError::Unhandled`].
pub fn read_state(path: &Path) -> Result<ModState> {
    let mut state = ModState::default();
    let Ok(bytes) = fs::read(path) else {
        return Ok(state);
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(t) => t,
        Err(e) => {
            return Err(ModsError::unhandled(
                "UnicodeDecodeError",
                pyfmt::utf8_error_text(&bytes, &e),
            ));
        }
    };
    let text = pyfmt::universal_newlines(text);
    for line in pyfmt::splitlines(&text) {
        let cols: Vec<&str> = line.split('\t').collect();
        let on = |c: &str| pyfmt::strip(c) == "1";
        if cols[0] == "mod" && cols.len() >= 3 {
            state.mods.insert(cols[1].to_string(), on(cols[2]));
        } else if (cols[0] == "patch" || cols[0] == "cheat") && cols.len() >= 4 {
            let key = (cols[1].to_string(), cols[2].to_string());
            if cols[0] == "patch" {
                state.patch.insert(key, on(cols[3]))
            } else {
                state.cheat.insert(key, on(cols[3]))
            };
        }
    }
    Ok(state)
}

pub fn state_path(docs: &Path, tid: &str) -> PathBuf {
    game_dir(docs, tid).join(STATE_NAME)
}

/// `list_mods`: the names of the stored mods, sorted (the order is the priority: the first one wins a file two mods both have).
pub fn list_mods(docs: &Path, tid: &str) -> Vec<String> {
    let d = mods_dir(docs, tid);
    let Ok(rd) = fs::read_dir(&d) else {
        return vec![];
    };
    let mut out: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && d.join(n).is_dir())
        .collect();
    out.sort();
    out
}

pub fn enabled_mods(docs: &Path, tid: &str) -> Result<Vec<String>> {
    let state = read_state(&state_path(docs, tid))?;
    Ok(list_mods(docs, tid)
        .into_iter()
        .filter(|m| state.mods.get(m).copied().unwrap_or(false))
        .collect())
}

/// `_files(base)`: every file under `base` as sorted paths relative to it with forward slashes, hidden files (a name that starts with a dot) left out.
/// `os.walk` semantics: a symbolic link to a folder is neither listed nor entered; an unreadable folder is skipped; a folder's name does not have to be
/// visible (a file inside a hidden folder is listed).
pub fn files(base: &Path) -> Vec<String> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut subdirs = Vec::new();
        for e in rd.filter_map(|e| e.ok()) {
            let Ok(name) = e.file_name().into_string() else {
                continue;
            };
            let path = e.path();
            // DirEntry.is_dir() follows symlinks; an error counts as "not a directory"
            if path.is_dir() {
                subdirs.push((name, path));
            } else if !name.starts_with('.') {
                out.push(if rel.is_empty() {
                    name
                } else {
                    format!("{rel}/{name}")
                });
            }
        }
        for (name, path) in subdirs {
            if fs::symlink_metadata(&path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                continue; // followlinks=False
            }
            walk(
                &path,
                &if rel.is_empty() {
                    name
                } else {
                    format!("{rel}/{name}")
                },
                out,
            );
        }
    }
    let mut out = Vec::new();
    walk(base, "", &mut out);
    out.sort();
    out
}

/// `_sha(path)`
pub fn sha(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path).map_err(|e| ModsError::Os(os_error(&e, path)))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| ModsError::Os(os_error(&e, path)))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// `exefs_signature`: what the last rewrite must have seen for the player's enabled mods: each enabled mod's exefs files with their digests, then the
/// patch-block choices of the enabled mods. The app computes the same text (`GameMods.ExefsSignature`) and says "Rebuild to apply" when it differs
/// from the one written at the last rewrite (`mods.applied`).
pub fn exefs_signature(docs: &Path, tid: &str) -> Result<String> {
    let state = read_state(&state_path(docs, tid))?;
    let mut lines: Vec<String> = Vec::new();
    for m in enabled_mods(docs, tid)? {
        let base = mods_dir(docs, tid).join(&m).join("exefs");
        if base.is_dir() {
            for rel in files(&base) {
                lines.push(format!("file\t{m}\t{rel}\t{}", sha(&base.join(&rel))?));
            }
        }
        for ((pm, name), on) in &state.patch {
            if *pm == m {
                lines.push(format!("patch\t{m}\t{name}\t{}", if *on { 1 } else { 0 }));
            }
        }
    }
    Ok(lines.join("\n"))
}
