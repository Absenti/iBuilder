//! The types every reader shares: a found shader, the per-job context, a file's scan result.

use crate::ctl::{KeyRules, St, check, same_shader, skey};
use crate::error::{Failure, Res};
use crate::unity::Trees;
use crate::zstd::Zstd;
use std::cell::RefCell;
use std::collections::HashMap;

/// `(where, stage, control section, code area)`: one shader found in a file
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shader {
    pub label: String,
    pub st: St,
    pub ctl: Vec<u8>,
    pub code: Vec<u8>,
}

impl Shader {
    pub fn new(label: impl Into<String>, st: St, ctl: &[u8], code: &[u8]) -> Shader {
        Shader {
            label: label.into(),
            st,
            ctl: ctl.to_vec(),
            code: code.to_vec(),
        }
    }
}

/// `check(label, st, ctl, code)` as the readers use it: `None` when the pair is consistent
pub fn why(st: St, ctl: &[u8], code: &[u8]) -> Res<Option<&'static str>> {
    check(st, ctl, code)
}

/// A pair `(vertex or mesh key, pixel key)` and whether it is a program-variation pair (BAHS, legacy Unity covers: `pso_pairs.tsv`)
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pair {
    pub a: String,
    pub b: String,
    pub prog: bool,
}

/// What `scan_file` returns beside the shaders: the Unity type trees this file carries, `'defer'`, or `('unreadable', why, count)`
#[derive(Clone, Debug)]
pub enum Extra {
    Trees(Trees),
    Defer,
    Unreadable { why: String, count: usize },
}

/// `(relative path, {key: shader} in first-seen order, pairs, extra)`
#[derive(Clone, Debug)]
pub struct FileResult {
    pub rel: String,
    pub uniq: Vec<(String, Shader)>,
    pub pairs: Vec<Pair>,
    pub extra: Extra,
}

impl FileResult {
    pub fn empty(rel: &str) -> FileResult {
        FileResult {
            rel: rel.to_string(),
            uniq: Vec::new(),
            pairs: Vec::new(),
            extra: Extra::Trees(Trees::new()),
        }
    }
}

/// What a scan job needs besides the bytes: the profile's key rule, the zstd, Unity's common strings and borrowed trees, the dictionaries, the log, and the files of the roots
/// (the readers that go through a sibling file: `.dict` beside `.data`, `.pkiswitch` beside `.pkdswitch`, ...). One per job (the BAHS variation labels are local to it).
pub struct Ctx<'a> {
    pub rules: &'a KeyRules,
    pub zstd: &'a dyn Zstd,
    pub common: Option<&'a HashMap<u32, String>>,
    /// `TREES`: the Shader / ComputeShader type trees borrowed in the second pass (empty in the first)
    pub borrowed: &'a Trees,
    /// `ZDICTS`: zstd dictionaries by ID
    pub zdicts: &'a HashMap<u32, Vec<u8>>,
    pub log: &'a (dyn Fn(&str) + Sync),
    pub files: &'a crate::split::FileIndex,
    /// `BAHS_VAR`: BAHS entry label to the program variation it belongs to
    pub bahs_var: RefCell<HashMap<String, String>>,
}

impl<'a> Ctx<'a> {
    pub fn skey(&self, ctl: &[u8], code: &[u8]) -> Res<String> {
        skey(self.rules, ctl, code)
    }
    pub fn same_shader(&self, a: &Shader, b: &Shader) -> Res<bool> {
        same_shader(self.rules, (&a.ctl, &a.code), (&b.ctl, &b.code))
    }
    pub fn print(&self, line: &str) {
        (self.log)(line)
    }
}

/// `uniq_keys(found)`: one entry per key, the first copy; a key whose two copies differ is `red()`
pub fn uniq_keys(cx: &Ctx<'_>, found: Vec<Shader>) -> Res<Vec<(String, Shader)>> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut uniq: Vec<(String, Shader)> = Vec::new();
    for s in found {
        let key = cx.skey(&s.ctl, &s.code)?;
        if let Some(&i) = index.get(&key) {
            if !cx.same_shader(&uniq[i].1, &s)? {
                return Err(Failure::Red(format!(
                    "key {key} is the same but the control section or code area differs ({})",
                    s.label
                )));
            }
            continue;
        }
        index.insert(key.clone(), uniq.len());
        uniq.push((key, s));
    }
    Ok(uniq)
}
