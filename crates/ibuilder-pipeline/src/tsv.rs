//! The tab-separated tables the chain reads: `profile.tsv` (`KEY<TAB>VALUE`, the runtime profile this run boots), the game table
//! `game.tsv` (the engine's `dm/games/game.py` `Game` class) and the shader map.

use crate::util::{is_file, parse_int0, read_text};
use std::path::{Path, PathBuf};

/// `_profile_title_id`: the `title_id` row of the profile, upper-cased; `None` when it is not there (or empty).
pub fn profile_title_id(profile: &Path) -> Option<String> {
    let text = read_text(profile).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("title_id\t") {
            let v = rest.trim().to_uppercase();
            return if v.is_empty() { None } else { Some(v) };
        }
    }
    None
}

/// `profile_hex`: a `KEY<TAB>0x...` value as an int, or 0 (unreadable, missing, or not a number: the first row of the key decides).
pub fn profile_hex(profile: &Path, key: &str) -> i128 {
    let Ok(text) = read_text(profile) else {
        return 0;
    };
    let prefix = format!("{key}\t");
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(&prefix) {
            let value = rest.split('#').next().unwrap_or("").trim();
            return parse_int0(value).unwrap_or(0);
        }
    }
    0
}

/// `_map_rows`: the rows of a shader map (not blank, not `#` comments, not the `package` header); -1 when the file is missing.
pub fn map_rows(path: &Path) -> i64 {
    match read_text(path) {
        Ok(t) => t.lines().filter(|l| is_map_row(l)).count() as i64,
        Err(_) => -1,
    }
}

fn is_map_row(l: &str) -> bool {
    !l.trim().is_empty() && !l.starts_with('#') && !l.starts_with("package")
}

/// `map_rows(path) > 0`, reading the map only up to its first row (a shader map has 22,000-43,000 of them, and each start and each stage 3 asked whether it had one; review r4 §6.3).
/// A file that cannot be opened or read has none.
pub fn map_has_rows(path: &Path) -> bool {
    use std::io::BufRead;
    let Ok(f) = std::fs::File::open(path) else {
        return false;
    };
    let mut r = std::io::BufReader::new(f);
    let mut line = Vec::new();
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return false,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&line);
        let l = text.strip_suffix('\n').unwrap_or(&text);
        if is_map_row(l.strip_suffix('\r').unwrap_or(l)) {
            return true;
        }
    }
}

/// The engine's `Game`: the rows of `game.tsv` (`key<TAB>values...`, `#` lines are comments, keys such as `build` repeat).
#[derive(Debug, Clone)]
pub struct Game {
    pub dir: PathBuf,
    pub rows: Vec<(String, Vec<String>)>,
}

impl Game {
    pub fn load(dir: &Path) -> std::io::Result<Game> {
        let text = read_text(&dir.join("game.tsv"))?;
        Ok(Game::parse(dir, &text))
    }

    pub fn parse(dir: &Path, text: &str) -> Game {
        let mut rows = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let mut cols = line.split('\t');
            let key = cols.next().unwrap_or("").trim().to_string();
            rows.push((key, cols.map(|c| c.trim().to_string()).collect()));
        }
        Game {
            dir: dir.to_path_buf(),
            rows,
        }
    }

    /// `g.get(key)`: the first row's values joined by tabs; `Err` carries the KeyError text.
    pub fn get(&self, key: &str) -> Result<String, String> {
        match self.rows.iter().find(|(k, _)| k == key) {
            Some((_, v)) => Ok(v.join("\t")),
            None => Err(format!(
                "{} has no key {key}",
                self.dir.join("game.tsv").display()
            )),
        }
    }

    /// `g.get(key, default)`
    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.get(key).unwrap_or_else(|_| default.to_string())
    }

    /// `g.int(key)` = `int(g.get(key), 0)`
    pub fn int(&self, key: &str) -> Result<i128, String> {
        let v = self.get(key)?;
        parse_int0(&v).ok_or_else(|| format!("invalid literal for int() with base 0: '{v}'"))
    }

    /// `int(g.get(key, default), 0)`
    pub fn int_or(&self, key: &str, default: &str) -> Result<i128, String> {
        let v = self.get_or(key, default);
        parse_int0(&v).ok_or_else(|| format!("invalid literal for int() with base 0: '{v}'"))
    }
}

/// The first `name<TAB>value` row of a table's text whose value (up to a `#`) is not empty, as an int; `default` when the name is not
/// there or the value is not a number (`presize_heap`'s `key()`: a regex `^name\t([^\n#]+)` and an `int(.., 0)` that falls back on ValueError).
pub fn table_int(text: &str, name: &str, default: i128) -> i128 {
    let prefix = format!("{name}\t");
    for line in text.split('\n') {
        if let Some(rest) = line.strip_prefix(&prefix) {
            let value = rest.split('#').next().unwrap_or("");
            if value.is_empty() {
                continue; // `[^\n#]+` needs at least one character: the regex goes on to the next line
            }
            return parse_int0(value).unwrap_or(default);
        }
    }
    default
}

/// `_last_guest_run`: the guest log of the last boot of game `tid` (from its `[dm] build: ... profile <tid>` line on), or "". The log is searched from its end (`rfind_line`): it holds every
/// boot of every game, and only what follows the last boot of this one is read (review r4 §6.3).
pub fn last_guest_run(guest_log: &Path, tid: &str) -> String {
    let needle = format!("profile {tid}");
    let found = crate::util::rfind_line(guest_log, |body| {
        let i = body.find("[dm] build: ")?;
        body[i + "[dm] build: ".len()..]
            .contains(&needle)
            .then_some(i)
    });
    match found {
        Ok(Some((i, line_start))) => crate::util::read_text_from(guest_log, line_start)
            .map(|t| t.get(i..).unwrap_or("").to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// `game_table_dir`: the folder holding this game's profile table (`game.tsv`): the one this app derived for a game the engine does not
/// know (`Documents/profile-derived/<id>/`), else the engine's own table shipped in the bundle (`dm/games/<id>/`). `None` if neither exists.
pub fn game_table_dir(derived_root: &Path, bundle: &Path, tid: &str) -> Option<PathBuf> {
    if tid.is_empty() {
        return None;
    }
    [
        derived_root.join(tid),
        bundle.join("dm").join("games").join(tid),
    ]
    .into_iter()
    .find(|d| is_file(&d.join("game.tsv")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TempDir, write};

    /// Review r4 §6.3: read from the end, `last_guest_run` gives what the whole read gave (several games' boots over many blocks, the game's last boot deep in the file or in a line that
    /// is longer than a block, a damaged byte before the match, no boot of the game at all).
    #[test]
    fn the_last_guest_run_read_from_the_end_is_the_whole_reads() {
        fn whole(guest_log: &Path, tid: &str) -> String {
            let Ok(text) = read_text(guest_log) else {
                return String::new();
            };
            let needle = format!("profile {tid}");
            let (mut last, mut pos) = (None, 0usize);
            for line in text.split_inclusive('\n') {
                let body = line.strip_suffix('\n').unwrap_or(line);
                if let Some(i) = body.find("[dm] build: ")
                    && body[i + "[dm] build: ".len()..].contains(&needle)
                {
                    last = Some(pos + i);
                }
                pos += line.len();
            }
            last.map(|i| text[i..].to_string()).unwrap_or_default()
        }
        let d = TempDir::new("guestrun");
        let p = d.join("gameapp.guest.log");
        let frames = |n: usize| "[guest] frame drawn in 16.6 ms\n".repeat(n);
        let long = "L".repeat(200_000);
        let cases: Vec<Vec<u8>> = vec![
            format!("[dm] build: x profile 0100DFA027504000\n{}[dm] build: x profile 0100000000010000\n{}", frames(5000), frames(9000)).into_bytes(),
            format!("{}[dm] build: x profile 0100DFA027504000\n{}", frames(9000), frames(3)).into_bytes(),
            [frames(4000).as_bytes(), b"\xff\xfe junk [dm] build: x profile 0100DFA027504000 more\n", frames(4000).as_bytes()].concat(),
            format!("{long} [dm] build: profile 0100DFA027504000 {long}\n{}", frames(10)).into_bytes(),
            format!("[dm] build: x profile 0100000000010000\n{}", frames(20_000)).into_bytes(),
            Vec::new(),
        ];
        for (i, c) in cases.iter().enumerate() {
            std::fs::write(&p, c).unwrap();
            assert_eq!(
                last_guest_run(&p, "0100DFA027504000"),
                whole(&p, "0100DFA027504000"),
                "case {i}"
            );
        }
        assert_eq!(last_guest_run(&d.join("missing"), "0100DFA027504000"), "");
    }

    /// Review r4 §6.3: `map_has_rows` is `map_rows > 0` without reading past the first row.
    #[test]
    fn a_map_has_rows_exactly_when_it_counts_some() {
        let d = TempDir::new("maprows");
        let p = d.join("shader_map.tsv");
        let rows: String = (0..30_000)
            .map(|i| format!("pkg{i}\t{i}\tvs\t{i:016x}\tsrc\n"))
            .collect();
        let cases = [
            String::new(),
            "package\tindex\tstage\tdm_ctl_sha16\tsource\n".to_string(),
            "package\tindex\n# a comment\n   \n\r\n".to_string(),
            "package\tindex\n#c\nx".to_string(),
            "x\r\n".to_string(),
            format!("package\tindex\n{rows}"),
            " # not a comment: the line starts with a space\n".to_string(),
        ];
        for (i, c) in cases.iter().enumerate() {
            write(&p, c);
            assert_eq!(map_has_rows(&p), map_rows(&p) > 0, "case {i}: {c:?}");
        }
        assert!(!map_has_rows(&d.join("missing")) && map_rows(&d.join("missing")) == -1);
        assert!(!map_has_rows(d.path()), "a folder has no rows");
    }

    #[test]
    fn title_id_and_hex() {
        let d = TempDir::new("tsv");
        let p = d.join("profile.tsv");
        write(
            &p,
            "# profile\ntitle_id\t0100bbbb00000000 \nmem_reserve\t0x140000000   # five\n",
        );
        assert_eq!(profile_title_id(&p).as_deref(), Some("0100BBBB00000000"));
        assert_eq!(profile_hex(&p, "mem_reserve"), 0x1_4000_0000);
        assert_eq!(profile_hex(&p, "nope"), 0);
        write(&p, "title_id\t\n");
        assert_eq!(profile_title_id(&p), None);
        assert_eq!(profile_title_id(&d.join("missing")), None);
    }

    #[test]
    fn table_int_follows_the_regex() {
        let t = "title_id\tX\n# mem_alias 0x5\nmem_alias\t0x118000000\nempty\t# note\nempty\t7\nbad\tzz\n";
        assert_eq!(table_int(t, "mem_alias", 0), 0x1_1800_0000);
        assert_eq!(
            table_int(t, "empty", 0),
            7,
            "an empty value does not match; the next row does"
        );
        assert_eq!(table_int(t, "bad", 9), 9);
        assert_eq!(table_int(t, "missing", 3), 3);
    }

    #[test]
    fn last_guest_run_starts_at_the_last_build_line_of_this_game() {
        let d = TempDir::new("guestrun");
        let p = d.join("g.log");
        let tid = "010043600B6A6000";
        write(
            &p,
            format!(
                "[dm] build: x, profile 0100DFA027504000\nold\n[dm] build: y, profile {tid}\nmid\n[dm] build: z, profile 0100DFA027504000\nother game\n[dm] build: w, profile {tid}\nlast\n"
            ),
        );
        assert_eq!(
            last_guest_run(&p, tid),
            format!("[dm] build: w, profile {tid}\nlast\n")
        );
        assert_eq!(last_guest_run(&p, "0000000000000000"), "");
        assert_eq!(last_guest_run(&d.join("none"), tid), "");
    }

    #[test]
    fn game_rows() {
        let g = Game::parse(
            Path::new("/x/0100"),
            "# c\nmem_reserve\t0x140000000\nbuild\ta\tb\nbuild\tc\nname\tGrizzy\t# not a comment col\n",
        );
        assert_eq!(g.get("mem_reserve").unwrap(), "0x140000000");
        assert_eq!(g.get("build").unwrap(), "a\tb");
        assert!(g.get("nope").is_err());
        assert_eq!(g.get_or("nope", "d"), "d");
        assert_eq!(g.int("mem_reserve").unwrap(), 0x1_4000_0000);
    }
}
