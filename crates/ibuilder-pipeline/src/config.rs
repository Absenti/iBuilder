//! `Documents/gameapp.config`: `key=value` lines the app rewrites every run (GameAppMain.ConfigText). `#` lines and lines without `=` are ignored.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config(BTreeMap<String, String>);

impl Config {
    pub fn new() -> Config {
        Config::default()
    }

    /// `read_config`: strip each line; skip empty, `#` and `=`-less lines; split at the first `=`, strip both sides.
    pub fn parse(text: &str) -> Config {
        let mut m = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || !line.contains('=') {
                continue;
            }
            let (k, v) = line.split_once('=').unwrap();
            m.insert(k.trim().to_string(), v.trim().to_string());
        }
        Config(m)
    }

    /// A missing or unreadable file reads as an empty config, as in Python (`os.path.isfile` first).
    pub fn read(path: &Path) -> Config {
        match fs::read(path) {
            Ok(b) => Config::parse(&String::from_utf8_lossy(&b)),
            Err(_) => Config::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(|s| s.as_str())
    }

    /// `cfg.get(key, default)`
    pub fn get_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).unwrap_or(default)
    }

    /// `cfg.get(key)` as Python truthiness: set and non-empty.
    pub fn get_nonempty(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|s| !s.is_empty())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.0.insert(key.to_string(), value.into());
    }

    pub fn set_default(&mut self, key: &str, value: impl Into<String>) {
        self.0
            .entry(key.to_string())
            .or_insert_with(|| value.into());
    }

    pub fn remove(&mut self, key: &str) {
        self.0.remove(key);
    }

    /// `cfg.get(key, default) != '0'`
    pub fn not_zero(&self, key: &str, default: &str) -> bool {
        self.get_or(key, default) != "0"
    }

    /// `int(cfg.get(key, default))`; `None` where Python raises ValueError.
    pub fn int(&self, key: &str, default: i64) -> Option<i64> {
        match self.get(key) {
            None => Some(default),
            Some(s) => s.trim().parse::<i64>().ok(),
        }
    }

    /// `int(cfg.get(key, default))` with a bad value falling back to `default` (the callers that wrapped it in try/except).
    pub fn int_or(&self, key: &str, default: i64) -> i64 {
        self.int(key, default).unwrap_or(default)
    }

    /// The `modules` row: `cfg.get('modules', 'main subsdk0 sdk').split()`
    pub fn modules(&self) -> Vec<String> {
        self.get_or("modules", "main subsdk0 sdk")
            .split_whitespace()
            .map(String::from)
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.0.iter()
    }
}

impl std::fmt::Display for Config {
    /// Python's `repr(dict)`, close enough for the log line `config <path>: {...}`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("{")?;
        for (i, (k, v)) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "'{k}': '{v}'")?;
        }
        f.write_str("}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_matches_read_config() {
        let c = Config::parse(
            "# comment\n\nextract_out = /x/extract \nnoequals\nprofile=/p=q\n  modules=main sdk  \n",
        );
        assert_eq!(c.get("extract_out"), Some("/x/extract"));
        assert_eq!(c.get("profile"), Some("/p=q"), "only the first = splits");
        assert_eq!(c.modules(), vec!["main", "sdk"]);
        assert_eq!(c.get("noequals"), None);
        assert_eq!(Config::read(Path::new("/no/such/file")), Config::new());
    }

    #[test]
    fn typed_getters() {
        let c = Config::parse("shader_jobs=4\nx=zz\nshader_cache=0");
        assert_eq!(c.int("shader_jobs", 0), Some(4));
        assert_eq!(c.int("x", 0), None);
        assert_eq!(c.int_or("x", 7), 7);
        assert!(!c.not_zero("shader_cache", "1"));
        assert!(c.not_zero("missing", "1"));
    }
}
