//! The build chain's hooks (stage 2): what the enabled mods' exefs patches do to each module, the patched copy the rewrite reads, and the note of what a
//! rewrite saw.

use crate::error::{ModsError, Result};
use crate::nso::{nso_build_id, patch_nso};
use crate::patch::{Patch, bid_matches, exefs_patches};
use crate::pyfmt::{self, os_error};
use crate::store::{exefs_signature, game_dir};
use crate::{APPLIED_NAME, BUILD_IDS_NAME, NSO_HEADER, PATCHED_DIR};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// A module name from the configuration (`modules`) is joined to the ExeFS folder and to `mods-exefs/`: only a plain file name is (not empty, not
/// `.` or `..`, no `/`, `\` or NUL), so a hand-edited configuration cannot make the chain read or write outside them. The Python joins it as it is.
pub fn is_module_name(m: &str) -> bool {
    !m.is_empty() && m != "." && m != ".." && !m.contains(['/', '\\', '\0'])
}

/// The log of the Python functions (`log=print`): one line per call.
pub type Log<'a> = &'a mut dyn FnMut(&str);

/// What `plan` found for one module: the digest of the patches that name it (the rewrite cache key's input) and those patches.
#[derive(Debug, Clone)]
pub struct PlanEntry {
    pub digest: String,
    pub patches: Vec<Patch>,
}

/// `{module: (digest, [Patch])}` for the modules some patch names by build id, in the order of the module list.
#[derive(Debug, Clone, Default)]
pub struct ModPlan {
    pub modules: Vec<(String, PlanEntry)>,
}

impl ModPlan {
    pub fn get(&self, module: &str) -> Option<&PlanEntry> {
        self.modules
            .iter()
            .find(|(m, _)| m == module)
            .map(|(_, e)| e)
    }
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

/// `record_build_ids`: `Documents/games/<id>/build_ids.txt`: `<module>\t<build id hex>` of the NSOs in ExeFS, which the importer reads to keep only what
/// matches this game (ExeFS is removed after a build, so the ids are kept here). A file name that is not ASCII is a `UnicodeEncodeError` in Python (`open(path,
/// 'w', encoding='ascii')`), which propagates.
pub fn record_build_ids(exe_dir: &Path, path: &Path, log: Log) -> Result<()> {
    let Ok(rd) = fs::read_dir(exe_dir) else {
        return Ok(());
    };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    let mut rows = Vec::new();
    for n in names {
        let Ok(f) = fs::File::open(exe_dir.join(&n)) else {
            continue;
        };
        let mut head = Vec::new();
        if f.take(NSO_HEADER as u64).read_to_end(&mut head).is_err() {
            continue;
        }
        if let Some(bid) = nso_build_id(&head) {
            rows.push(format!("{n}\t{}", pyfmt::hex_upper(bid)));
        }
    }
    if rows.is_empty() {
        return Ok(());
    }
    let text = rows.join("\n") + "\n";
    match fs::File::create(path) {
        Ok(mut f) => {
            if !text.is_ascii() {
                return Err(ModsError::unhandled(
                    "UnicodeEncodeError",
                    "'ascii' codec can't encode characters in a build id row",
                ));
            }
            use std::io::Write;
            if let Err(e) = f.write_all(text.as_bytes()) {
                log(&format!(
                    "mods: build ids not recorded ({})",
                    os_error(&e, path)
                ));
            }
        }
        Err(e) => log(&format!(
            "mods: build ids not recorded ({})",
            os_error(&e, path)
        )),
    }
    Ok(())
}

/// `plan`: what the enabled mods' exefs patches do to each module: `{module: (digest, [Patch])}` for the modules some patch names by build id. Only the NSO
/// headers are read, so it is cheap: the rewrite cache's key needs it for every module, and the patched copy (`apply_module`) is written only when the key
/// misses. A bad mod is logged (unless `quiet`) and the module is rewritten as dumped. Also records the build ids (`build_ids.txt`) for the importer.
///
/// Errors: only what the Python lets through: a `struct.error` from `Patch.digest` of a patch that writes below 0x100 (an IPS offset under the header size,
/// a pchtxt address under 0x100 with no shift), a `ValueError` from a pchtxt address that is not a number after all, a `mods.tsv` that is not UTF-8, an
/// unreadable exefs file.
pub fn plan(
    docs: &Path,
    tid: &str,
    exe_dir: &Path,
    modules: &[String],
    log: Log,
    quiet: bool,
) -> Result<ModPlan> {
    let mut out = ModPlan::default();
    if tid.is_empty() {
        return Ok(out);
    }
    if !quiet {
        record_build_ids(exe_dir, &game_dir(docs, tid).join(BUILD_IDS_NAME), log)?;
    }
    let (patches, skipped) = exefs_patches(docs, tid, None)?;
    if !quiet {
        for (m, rel, why) in &skipped {
            log(&format!("mods: {m}/exefs/{rel} not used: {why}"));
        }
    }
    let mut matched: BTreeSet<(String, String, String)> = BTreeSet::new();
    for m in modules {
        if !is_module_name(m) {
            if !quiet {
                log(&format!(
                    "mods: module {m:?} is not a file name: not patched"
                ));
            }
            continue;
        }
        let Ok(f) = fs::File::open(exe_dir.join(m)) else {
            continue;
        };
        let mut head = Vec::new();
        if f.take(NSO_HEADER as u64).read_to_end(&mut head).is_err() {
            continue;
        }
        let Some(bid) = nso_build_id(&head) else {
            continue;
        };
        let mine: Vec<&Patch> = patches
            .iter()
            .filter(|p| p.on && bid_matches(&p.bid, bid))
            .collect();
        if mine.is_empty() {
            continue;
        }
        let mut h = Sha256::new();
        for p in &mine {
            h.update(p.digest()?.as_bytes());
            matched.insert((p.mod_name.clone(), p.source.clone(), p.name.clone()));
        }
        let entry = PlanEntry {
            digest: h.finalize().iter().map(|b| format!("{b:02x}")).collect(),
            patches: mine.into_iter().cloned().collect(),
        };
        match out.modules.iter_mut().find(|(n, _)| n == m) {
            Some(slot) => slot.1 = entry,
            None => out.modules.push((m.clone(), entry)),
        }
    }
    if !quiet {
        for p in &patches {
            if p.on && !matched.contains(&(p.mod_name.clone(), p.source.clone(), p.name.clone())) {
                log(&format!(
                    "mods: {}/{} [{}] matches none of this game's modules (another build of the game?): not applied",
                    p.mod_name, p.source, p.name
                ));
            }
        }
    }
    Ok(out)
}

/// `apply_module`: the patched copy of module `m` (`Documents/games/<id>/mods-exefs/<m>`), or `None` when it cannot be made (logged: the module is then
/// rewritten as dumped, and the sheet's rebuild note stays). Not caught, as in Python: an `LZ4BlockError` of a damaged segment, a failure to write the copy.
pub fn apply_module(
    docs: &Path,
    tid: &str,
    exe_dir: &Path,
    m: &str,
    patches: &[Patch],
    log: Log,
) -> Result<Option<PathBuf>> {
    if !is_module_name(m) {
        log(&format!(
            "mods: module {m:?} is not a file name: left as dumped"
        ));
        return Ok(None);
    }
    let src = exe_dir.join(m);
    let patched = fs::read(&src)
        .map_err(|e| ModsError::Os(os_error(&e, &src)))
        .and_then(|data| {
            patch_nso(&data, patches).map(|r| {
                r.map(|p| {
                    (
                        p.data,
                        p.applied.into_iter().cloned().collect::<Vec<Patch>>(),
                        p.notes
                            .into_iter()
                            .map(|(p, n)| (p.clone(), n))
                            .collect::<Vec<_>>(),
                    )
                })
            })
        });
    let (new, applied, notes) = match patched {
        Ok(None) => return Ok(None),
        Ok(Some(t)) => t,
        Err(e) if e.is_mod() || e.is_os() => {
            log(&format!("mods: {m} left as dumped: {e}"));
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let out_dir = game_dir(docs, tid).join(PATCHED_DIR);
    fs::create_dir_all(&out_dir).map_err(|e| ModsError::Os(os_error(&e, &out_dir)))?;
    let dst = out_dir.join(m);
    // written beside it, then renamed over it: a copy cut short (disk full, the app killed) never stands under the module's name, and a `dst` that is
    // a hard link (a cached tree) is replaced, not written through. The rename replaces an older copy itself: nothing is deleted first.
    let tmp = out_dir.join(format!(".{m}.part"));
    if let Err(e) = fs::write(&tmp, &new) {
        let _ = fs::remove_file(&tmp);
        return Err(ModsError::Os(os_error(&e, &dst)));
    }
    if let Err(e) = fs::rename(&tmp, &dst) {
        let _ = fs::remove_file(&tmp);
        return Err(ModsError::Os(os_error(&e, &dst)));
    }
    log(&format!(
        "mods: {m} patched: {}",
        applied
            .iter()
            .map(|p| p.label())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    for (p, outside) in &notes {
        log(&format!(
            "mods: {}: {outside} write(s) fall outside the module's text/rodata/data (the header, bss or a gap): not made",
            p.label()
        ));
    }
    Ok(Some(dst))
}

pub fn remove_patched(docs: &Path, tid: &str) {
    if !tid.is_empty() {
        let _ = fs::remove_dir_all(game_dir(docs, tid).join(PATCHED_DIR));
    }
}

/// `mark_applied`: after a rewrite, remember what it saw, for the sheet's "Rebuild to apply". The file is opened (emptied) before the signature is computed,
/// as in Python: a failure inside leaves it empty.
pub fn mark_applied(docs: &Path, tid: &str, log: Log) -> Result<()> {
    if tid.is_empty() {
        return Ok(());
    }
    let path = game_dir(docs, tid).join(APPLIED_NAME);
    let run = || -> Result<()> {
        let mut f = fs::File::create(&path).map_err(|e| ModsError::Os(os_error(&e, &path)))?;
        let sig = exefs_signature(docs, tid)?;
        use std::io::Write;
        f.write_all(sig.as_bytes())
            .map_err(|e| ModsError::Os(os_error(&e, &path)))
    };
    match run() {
        Err(ModsError::Os(e)) => {
            log(&format!("mods: could not write {APPLIED_NAME} ({e})"));
            Ok(())
        }
        other => other,
    }
}
