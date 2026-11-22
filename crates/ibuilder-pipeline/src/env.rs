//! The environment the engine's host reads (`getenv("DM_SAVE")`, `DM_NVN_SHMAP`, ...). The Python chain mutated `os.environ`; here every
//! read and write goes through `Env`, so the chain is testable and thread-safe, and the process environment is touched exactly once, by
//! `ProcessEnv::commit()` right before the boot (the host is C code in this process and only sees the real environment).
//! `std::env::set_var` is `unsafe` in edition 2024 (it races with `getenv` on other threads); `commit` runs where the app is not
//! multi-threaded in the host's sense -- before the guest starts -- and is the one place that does it (`sys::setenv`).

use std::collections::BTreeMap;
use std::sync::Mutex;

pub trait Env: Send + Sync {
    /// `os.environ.get(name)`: `None` when unset; a set-but-empty variable is `Some("")` (Python distinguishes it too).
    fn get(&self, name: &str) -> Option<String>;
    fn set(&self, name: &str, value: &str);
    fn remove(&self, name: &str);
    /// Make what was set visible to C code in this process (the host's `getenv`). A no-op for environments that are only maps.
    fn commit(&self) {}
    /// Python's `if os.environ.get(name):`
    fn truthy(&self, name: &str) -> bool {
        self.get(name).map(|v| !v.is_empty()).unwrap_or(false)
    }
}

/// An environment that is only a map (tests; a CLI that does not boot).
#[derive(Default)]
pub struct MapEnv(Mutex<BTreeMap<String, String>>);

impl MapEnv {
    pub fn new() -> MapEnv {
        MapEnv::default()
    }
    pub fn with(pairs: &[(&str, &str)]) -> MapEnv {
        let e = MapEnv::new();
        for (k, v) in pairs {
            e.set(k, v);
        }
        e
    }
    pub fn snapshot(&self) -> BTreeMap<String, String> {
        self.0.lock().unwrap().clone()
    }
}

impl Env for MapEnv {
    fn get(&self, name: &str) -> Option<String> {
        self.0.lock().unwrap().get(name).cloned()
    }
    fn set(&self, name: &str, value: &str) {
        self.0
            .lock()
            .unwrap()
            .insert(name.to_string(), value.to_string());
    }
    fn remove(&self, name: &str) {
        self.0.lock().unwrap().remove(name);
    }
}

/// The process environment, written immediately (for the code that runs before any overlay exists: the early address-space reservation).
pub struct RealEnv;

impl Env for RealEnv {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
    }
    fn set(&self, name: &str, value: &str) {
        crate::sys::setenv(name, value);
    }
    fn remove(&self, name: &str) {
        crate::sys::unsetenv(name);
    }
}

/// The real process environment as a base, with the chain's writes held in an overlay until `commit`.
#[derive(Default)]
pub struct ProcessEnv {
    overlay: Mutex<BTreeMap<String, Option<String>>>,
}

impl ProcessEnv {
    pub fn new() -> ProcessEnv {
        ProcessEnv::default()
    }
    /// What the chain changed so far (`None` = removed).
    pub fn changes(&self) -> Vec<(String, Option<String>)> {
        self.overlay
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
    /// Apply the overlay to the process environment (what `os.environ[...] = ...` did immediately). Call before the guest boots.
    pub fn commit(&self) {
        let mut o = self.overlay.lock().unwrap();
        for (k, v) in o.iter() {
            match v {
                Some(v) => crate::sys::setenv(k, v),
                None => crate::sys::unsetenv(k),
            }
        }
        o.clear();
    }
}

impl Env for ProcessEnv {
    fn get(&self, name: &str) -> Option<String> {
        if let Some(v) = self.overlay.lock().unwrap().get(name) {
            return v.clone();
        }
        std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
    }
    fn set(&self, name: &str, value: &str) {
        self.overlay
            .lock()
            .unwrap()
            .insert(name.to_string(), Some(value.to_string()));
    }
    fn remove(&self, name: &str) {
        self.overlay.lock().unwrap().insert(name.to_string(), None);
    }
    fn commit(&self) {
        ProcessEnv::commit(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_env_distinguishes_unset_from_empty() {
        let e = MapEnv::new();
        assert_eq!(e.get("DM_X"), None);
        e.set("DM_X", "");
        assert_eq!(e.get("DM_X"), Some(String::new()));
        assert!(
            !e.truthy("DM_X"),
            "an empty value is falsy, as in `if os.environ.get(...)`"
        );
        e.set("DM_X", "1");
        assert!(e.truthy("DM_X"));
        e.remove("DM_X");
        assert_eq!(e.get("DM_X"), None);
    }

    #[test]
    fn process_env_overlay_wins_and_commits() {
        let e = ProcessEnv::new();
        let name = "IBUILDER_PIPELINE_TEST_OVERLAY";
        assert_eq!(e.get(name), None);
        e.set(name, "a");
        assert_eq!(e.get(name), Some("a".into()));
        assert!(
            std::env::var_os(name).is_none(),
            "nothing reaches the process until commit()"
        );
        e.commit();
        assert_eq!(std::env::var(name).unwrap(), "a");
        e.remove(name);
        e.commit();
        assert!(std::env::var_os(name).is_none());
    }
}
