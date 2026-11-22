//! The game profile `shader_split.py` reads (`game.load()`): `<profile folder>/<title id>/game.tsv`, key <TAB> values, `#` lines are comments.
//! It gives the source groups (`shader_groups`), the table header note (`shader_note`), the key rule (`shader_key_skip`) and the folder of the hand-written facts.

use crate::ctl::KeyRules;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Game {
    pub dir: PathBuf,
    pub id: String,
    rows: Vec<(String, Vec<String>)>,
}

impl Game {
    /// `shader_device._profile_env` + `game.load`: `profile_dir` is the game's own folder (it holds `game.tsv`) or the folder that holds `<game_id>/`
    pub fn load(profile_dir: &Path, game_id: &str) -> Result<Game, String> {
        let (parent, gid) = if profile_dir.join("game.tsv").is_file() {
            let abs = std::fs::canonicalize(profile_dir)
                .map_err(|e| format!("{}: {e}", profile_dir.display()))?;
            (
                abs.parent().map(Path::to_path_buf).unwrap_or_default(),
                abs.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
        } else {
            (profile_dir.to_path_buf(), game_id.to_string())
        };
        if gid != game_id || !parent.join(&gid).join("game.tsv").is_file() {
            return Err(format!(
                "{} holds no profile of game {game_id} (<dir>/{game_id}/game.tsv)",
                profile_dir.display()
            ));
        }
        Game::load_dir(&parent.join(&gid))
    }

    pub fn load_dir(dir: &Path) -> Result<Game, String> {
        let p = dir.join("game.tsv");
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let id = dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Game::from_text(dir, id, &text))
    }

    pub fn from_text(dir: PathBuf, id: String, text: &str) -> Game {
        let mut rows = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let c: Vec<&str> = line.split('\t').collect();
            rows.push((
                c[0].trim().to_string(),
                c[1..].iter().map(|v| v.trim().to_string()).collect(),
            ));
        }
        Game { dir, id, rows }
    }

    /// `get(key)`: the first row of this key, its values joined by a tab; None when there is no such row (the Python's `KeyError`)
    pub fn get(&self, key: &str) -> Option<String> {
        self.rows
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.join("\t"))
    }

    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.get(key).unwrap_or_else(|| default.to_string())
    }

    pub fn words(&self, key: &str) -> Option<Vec<String>> {
        self.get(key)
            .map(|s| s.split_whitespace().map(str::to_string).collect())
    }

    /// `pairs(key)`: `a=b c=d` to `[(a, b), (c, d)]`
    pub fn pairs(&self, key: &str) -> Result<Vec<(String, String)>, String> {
        let w = self
            .words(key)
            .ok_or_else(|| format!("{} has no key {key}", self.dir.join("game.tsv").display()))?;
        w.iter()
            .map(|w| {
                w.split_once('=')
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .ok_or_else(|| format!("{key}: {w:?} is not a=b"))
            })
            .collect()
    }

    pub fn table(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

/// What the split reads of a profile
#[derive(Clone, Debug)]
pub struct Profile {
    pub groups: Vec<(String, String)>,
    pub note: String,
    pub rules: KeyRules,
    pub manual_facts: PathBuf,
    pub game: Game,
}

impl Profile {
    pub fn from_game(game: Game) -> Result<Profile, String> {
        let groups = game.pairs("shader_groups")?;
        let note = game.get("shader_note").ok_or_else(|| {
            format!(
                "{} has no key shader_note",
                game.dir.join("game.tsv").display()
            )
        })?;
        let rules = KeyRules::parse(&game.get_or("shader_key_skip", "0+0"))?;
        let manual_facts = game.table("shader_facts_manual.tsv");
        Ok(Profile {
            groups,
            note,
            rules,
            manual_facts,
            game,
        })
    }

    /// `group(label)`: the source group (second half of the package file name)
    pub fn group(&self, label: &str) -> &str {
        if label.starts_with("main") {
            return "main";
        }
        for (k, g) in &self.groups {
            if label.contains(k.as_str()) {
                return g;
            }
        }
        "other"
    }
}
