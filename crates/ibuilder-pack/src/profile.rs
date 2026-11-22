//! The runtime profile TSV (`dm/games/game.py profile`; read by the host from DM_PROFILE, `hle_profile.c`).
use crate::pyutil::{py_lstrip, py_splitlines, py_strip};
use std::io;
use std::path::Path;

/// `KEY -> first value` (in file order, first wins) plus every `display_version <build> <ver>` row in order.
/// Comments and blank lines are skipped, exactly as `hle_profile.c` `set_kv` does.
#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub kv: Vec<(String, String)>,
    pub display_versions: Vec<(String, String)>,
}

impl Profile {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.kv
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

fn read_text_lossy(path: &Path) -> io::Result<String> {
    Ok(String::from_utf8_lossy(&std::fs::read(path)?).into_owned())
}

pub fn read_profile(path: &Path) -> io::Result<Profile> {
    let text = read_text_lossy(path)?;
    let mut p = Profile::default();
    for line in py_splitlines(&text) {
        if py_strip(line).is_empty() || py_lstrip(line).starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        let key = py_strip(cols[0]);
        if key == "display_version" && cols.len() >= 3 {
            p.display_versions
                .push((py_strip(cols[1]).to_string(), py_strip(cols[2]).to_string()));
        }
        if !p.kv.iter().any(|(k, _)| k == key) {
            p.kv.push((
                key.to_string(),
                if cols.len() > 1 {
                    py_strip(cols[1]).to_string()
                } else {
                    String::new()
                },
            ));
        }
    }
    Ok(p)
}

/// `switch2` for a Switch 2 game, `switch1` for any other, `None` without a readable profile: the first line names
/// it, `# <name> (Switch 2, ...)` (the engine's `gen_manifest.console_of`).
pub fn console_of(path: &Path) -> Option<&'static str> {
    let text = read_text_lossy(path).ok()?;
    let first = *py_splitlines(&text).first()?;
    Some(if first.starts_with('#') && first.contains("(Switch 2") {
        "switch2"
    } else {
        "switch1"
    })
}

/// Rows `gen_embed.py` embeds: everything but comments, blank lines and the `refused` ones (`nro_count`).
pub fn nro_count(table: &Path) -> io::Result<u64> {
    let text = String::from_utf8(std::fs::read(table)?)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(py_splitlines(&text)
        .into_iter()
        .map(|l| l.split('\t').next().unwrap_or(""))
        .filter(|n| !n.is_empty() && !n.starts_with('#') && *n != "refused")
        .count() as u64)
}
