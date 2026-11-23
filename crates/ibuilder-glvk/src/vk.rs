//! `vk_shaders.py`: a Vulkan game's SPIR-V shaders -> MSL + the Vulkan layer's table (`vk/vk_shaders.tsv`, read by the host's `dm/gfx/vk/hle_vk.m`).
//!
//! A port of the engine's tool function by function (same names); everything but its last step (`xcrun metal` / `metallib`): the MSL of every function the Python
//! passes to the Metal compiler is returned instead ([`Layer::msl`]). The table is byte-identical to the Python's (`tests/vk_oracle.rs`).
//!
//! * key = `sha256(SPIR-V words)[:16]`, Metal function `vk_<key>` (`_d`: the dual-source variant of a fragment shader, `_vp<n>`: a vertex shader that writes the
//!   viewport index a folded geometry stage routed from its output `n`);
//! * bindings renumbered per Metal slot class in a copy of the SPIR-V (`--msl-decoration-binding`), sets moved to argument buffers only past Metal's limits;
//! * a triangle pass-through geometry stage is proved by a small symbolic interpreter ([`geometry_viewport`]) and folded into the vertex stage (`geomvp:<n>`).
//!
//! A `red()` of the Python (or an exception it does not catch) is an `Err(Fatal)`: the whole run stops, as the Python's does.

use crate::py::isspace;
use crate::tools::{Tool, run};
use crate::walk::{self, Root};
use crate::{Api, Fatal, Layer, LayerTools, red};
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const MAGIC: u32 = 0x0723_0203;
const SPIRV_MAGIC: [u8; 4] = MAGIC.to_le_bytes();
const MATERIAL_MAGIC: [u8; 8] = 0x0A11_DA1Au64.to_le_bytes();
const OP_DECORATE: u32 = 71;
const OP_MEMBER_DECORATE: u32 = 72;
const OP_TYPE_ARRAY: u32 = 28;
const OP_TYPE_RTARRAY: u32 = 29;
const OP_TYPE_STRUCT: u32 = 30;
const DEC_ARRAY_STRIDE: u32 = 6;
const DEC_BINDING: u32 = 33;
const DEC_SET: u32 = 34;
const DEC_BUILTIN: u32 = 11;
const DEC_LOCATION: u32 = 30;
const POSITION: u32 = 0;
const INSTANCE_INDEX: u32 = 43;
const VIEWPORT_INDEX: u32 = 10;
/// `KINDS`, in the Python dict's order
const KINDS: [(&str, char); 6] = [
    ("ubos", 'u'),
    ("ssbos", 'b'),
    ("separate_images", 't'),
    ("images", 't'),
    ("separate_samplers", 's'),
    ("textures", 'c'),
];
/// Metal Feature Set Tables: buffers 0-30, textures 0-30, samplers 0-15
const LIMITS: [(Class, usize); 3] = [
    (Class::Buffer, 31),
    (Class::Texture, 31),
    (Class::Sampler, 16),
];
const HEADER: &str = "# Vulkan shaders (dm/gfx/tools/vk_shaders.py): key = sha256(SPIR-V)[:16], Metal function vk_<key>; stage vert/frag/comp:x,y,z/geomvp:location; resource kinds u/b/t/s/c/p and argument-buffer U/B/T/S/o/q";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Class {
    Buffer,
    Texture,
    Sampler,
}

impl Class {
    fn of(name: &str) -> Option<Class> {
        match name {
            "buffer" => Some(Class::Buffer),
            "texture" => Some(Class::Texture),
            "sampler" => Some(Class::Sampler),
            _ => None,
        }
    }
}

/// `SLOT_CLASS`
fn slot_class(kind: char) -> Option<Class> {
    match kind {
        'u' | 'b' | 'p' => Some(Class::Buffer),
        't' | 'c' => Some(Class::Texture),
        's' => Some(Class::Sampler),
        _ => None,
    }
}

/// `{'buffer': n, 'texture': n, 'sampler': n}` as the Python prints such a dict
#[derive(Clone, Copy, Default, Debug)]
struct Counts([i64; 3]);

impl Counts {
    fn get(&self, c: Class) -> i64 {
        self.0[c as usize]
    }
    fn add(&mut self, c: Class, n: i64) {
        self.0[c as usize] += n;
    }
    fn repr(&self) -> String {
        format!(
            "{{'buffer': {}, 'texture': {}, 'sampler': {}}}",
            self.0[0], self.0[1], self.0[2]
        )
    }
}

fn rx(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

fn fatal(what: impl std::fmt::Display) -> Fatal {
    Fatal(what.to_string())
}

fn index_error() -> Fatal {
    Fatal("IndexError: list index out of range".into())
}

fn word(w: &[u32], i: usize) -> Result<u32, Fatal> {
    w.get(i).copied().ok_or_else(index_error)
}

/// Python's `repr` of a JSON value (the messages that print the reflection)
fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => py_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", py_str(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn py_str(s: &str) -> String {
    if s.contains('\'') && !s.contains('"') {
        format!("\"{s}\"")
    } else {
        format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

/// `str.split()` without arguments
fn py_split(s: &str) -> Vec<&str> {
    s.split(isspace).filter(|x| !x.is_empty()).collect()
}

/// `s[:n]` of a `str`
fn head_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ---- inputs ------------------------------------------------------------------------------------------------------------------------------

/// `whole_module(data, start, end)`
fn whole_module(data: &[u8], start: usize, end: usize) -> bool {
    if end > data.len() || end < start || end - start < 24 || !(end - start).is_multiple_of(4) {
        return false;
    }
    if data[start..start + 4] != SPIRV_MAGIC || u32le(data, start + 16) != 0 {
        return false;
    }
    let mut pos = start + 20;
    while pos < end {
        let words = (u32le(data, pos) >> 16) as usize;
        if words == 0 {
            return false;
        }
        pos += words * 4;
    }
    pos == end
}

fn u32le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// `bgfx_modules(data)`: `(offset, SPIR-V)` of the size-delimited payload of every bgfx v5 graphics or v3 compute shader in `data`
fn bgfx_modules(data: &[u8]) -> Vec<(usize, Vec<u8>)> {
    static R: OnceLock<regex::bytes::Regex> = OnceLock::new();
    let r = R.get_or_init(|| regex::bytes::Regex::new(r"(?-u)[VF]SH\x05|CSH\x03").unwrap());
    let mut out = Vec::new();
    for m in r.find_iter(data) {
        let mut pos = m.end() + 4; // shader I/O hash
        if pos + 2 > data.len() {
            break;
        }
        let uniforms = u16::from_le_bytes([data[pos], data[pos + 1]]);
        pos += 2;
        for _ in 0..uniforms {
            if pos >= data.len() {
                break;
            }
            pos += 7 + data[pos] as usize; // name length, name, type, element count, register index and count
        }
        if pos + 4 > data.len() {
            continue;
        }
        let size = u32le(data, pos) as usize;
        let (start, end) = (pos + 4, pos + 4 + size);
        if whole_module(data, start, end) {
            out.push((start, data[start..end].to_vec()));
        }
    }
    out
}

/// One shader module of the inputs: `(key, where, SPIR-V bytes)`
pub type Source = (String, String, Vec<u8>);

/// The modules of one input file: `(offset in it, or None for a whole SPIR-V file, bytes)`
type Modules = Vec<(Option<usize>, Vec<u8>)>;

/// A translated shader: `(key, stage field, resources, [(msl file name, text)], where)`
type Done = (String, String, Vec<String>, Vec<(String, String)>, String);

/// `spirv_sources(roots)`: every SPIR-V module of the roots, deduplicated by key (the first one found keeps its path)
pub fn spirv_sources(roots: &[PathBuf]) -> Result<Vec<Source>, Fatal> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for root in roots {
        let mut modules: Vec<(String, Modules)> = Vec::new();
        match walk::root(root)? {
            Root::Missing => return red(format!("{}: no such file or folder", root.display())),
            Root::File(p) => {
                let data = std::fs::read(&p).map_err(|e| fatal(format!("{}: {e}", p.display())))?;
                if data.get(..4) == Some(b"NSO0") {
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let m = ibuilder_nso::load(&data, &p.to_string_lossy())
                        .map_err(|e| Fatal(format!("[RED] {e}")))?;
                    let found = bgfx_modules(&m.image)
                        .into_iter()
                        .map(|(o, d)| (Some(o), d))
                        .collect();
                    modules.push((name, found));
                }
            }
            Root::Tree(dirs) => {
                for d in dirs {
                    for f in d.files {
                        let head = f.head(8).map_err(|e| fatal(format!("{}: {e}", f.rel)))?;
                        let name = format!("/{}", f.rel);
                        if head.starts_with(&SPIRV_MAGIC) {
                            let data = f.read().map_err(|e| fatal(format!("{}: {e}", f.rel)))?;
                            modules.push((name, vec![(None, data)]));
                        } else if head == MATERIAL_MAGIC {
                            let data = f.read().map_err(|e| fatal(format!("{}: {e}", f.rel)))?;
                            let found = bgfx_modules(&data)
                                .into_iter()
                                .map(|(o, d)| (Some(o), d))
                                .collect();
                            modules.push((name, found));
                        }
                    }
                }
            }
        }
        for (name, ms) in modules {
            for (offset, data) in ms {
                let key = key_of(&data);
                if !seen.insert(key.clone()) {
                    continue;
                }
                let at = match offset {
                    None => name.clone(),
                    Some(o) => format!("{name}@{o:#x}"),
                };
                out.push((key, at, data));
            }
        }
    }
    Ok(out)
}

/// `hashlib.sha256(data).hexdigest()[:16]`
pub fn key_of(data: &[u8]) -> String {
    let d = Sha256::digest(data);
    d[..8].iter().map(|b| format!("{b:02x}")).collect()
}

// ---- SPIR-V rewrites ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Default, Debug)]
struct Dec {
    set: Option<u32>,
    binding: Option<u32>,
    binding_at: Option<usize>,
}

/// `decorations(words)`: variable id -> its set, binding and the word index of the binding value, in the order of the first decoration of each id
fn decorations(words: &[u32]) -> Result<Vec<(u32, Dec)>, Fatal> {
    let mut out: Vec<(u32, Dec)> = Vec::new();
    let mut at: HashMap<u32, usize> = HashMap::new();
    let mut i = 5;
    while i < words.len() {
        let (op, n) = (words[i] & 0xffff, (words[i] >> 16) as usize);
        if n == 0 {
            return red("SPIR-V instruction length 0");
        }
        if op == OP_DECORATE && n >= 4 {
            let id = word(words, i + 1)?;
            let k = *at.entry(id).or_insert_with(|| {
                out.push((id, Dec::default()));
                out.len() - 1
            });
            let d = word(words, i + 2)?;
            if d == DEC_BINDING {
                out[k].1.binding = Some(word(words, i + 3)?);
                out[k].1.binding_at = Some(i + 3);
            } else if d == DEC_SET {
                out[k].1.set = Some(word(words, i + 3)?);
            }
        }
        i += n;
    }
    Ok(out)
}

/// `fix_strides(words)`: an ArrayStride put on the struct member (OpMemberDecorate) instead of the array type is copied onto the array type
pub fn fix_strides(words: Vec<u32>) -> Result<Vec<u32>, Fatal> {
    let mut arrays = HashSet::new();
    let mut have = HashSet::new();
    let mut member: Vec<(u32, u32, u32)> = Vec::new();
    let mut members: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut at = None;
    let mut i = 5;
    while i < words.len() {
        let (op, n) = (words[i] & 0xffff, (words[i] >> 16) as usize);
        if n == 0 {
            return red("SPIR-V instruction length 0");
        }
        if op == OP_DECORATE && n >= 4 && word(&words, i + 2)? == DEC_ARRAY_STRIDE {
            have.insert(word(&words, i + 1)?);
        } else if op == OP_MEMBER_DECORATE && n >= 5 && word(&words, i + 3)? == DEC_ARRAY_STRIDE {
            member.push((
                word(&words, i + 1)?,
                word(&words, i + 2)?,
                word(&words, i + 4)?,
            ));
            at = Some(i + n);
        } else if op == OP_TYPE_ARRAY || op == OP_TYPE_RTARRAY {
            arrays.insert(word(&words, i + 1)?);
        } else if op == OP_TYPE_STRUCT {
            let id = word(&words, i + 1)?;
            members.insert(
                id,
                words[(i + 2).min(words.len())..(i + n).min(words.len())].to_vec(),
            );
        }
        i += n;
    }
    let mut add: Vec<(u32, u32)> = Vec::new();
    for (st, m, stride) in member {
        let ms = members.get(&st).map(|v| v.as_slice()).unwrap_or(&[]);
        let t = ms.get(m as usize).copied();
        if let Some(t) = t
            && arrays.contains(&t)
            && !have.contains(&t)
            && !add.iter().any(|(a, _)| *a == t)
        {
            add.push((t, stride));
        }
    }
    if add.is_empty() {
        return Ok(words);
    }
    let at = at.unwrap().min(words.len());
    let mut out = words[..at].to_vec();
    for (t, s) in add {
        out.extend_from_slice(&[(4 << 16) | OP_DECORATE, t, DEC_ARRAY_STRIDE, s]);
    }
    out.extend_from_slice(&words[at..]);
    Ok(out)
}

/// `unsigned_bone_indices(words)`: RenderDragon's scalar `int a_indices` vertex input becomes unsigned, each load followed by an OpBitcast under the old signed id
pub fn unsigned_bone_indices(words: Vec<u32>) -> Result<Vec<u32>, Fatal> {
    let mut instructions: Vec<Vec<u32>> = Vec::new();
    let mut names = HashSet::new();
    let mut ints: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut pointers: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut variables: HashMap<u32, (u32, u32)> = HashMap::new();
    let mut models = Vec::new();
    let mut locations = HashSet::new();
    let mut interface = HashSet::new();
    let mut i = 5;
    while i < words.len() {
        let (n, op) = ((words[i] >> 16) as usize, words[i] & 0xffff);
        if n == 0 {
            return red("SPIR-V instruction length 0");
        }
        let ins = words[i..(i + n).min(words.len())].to_vec();
        match op {
            5 if crate::spv::literal(ins.get(2..).unwrap_or(&[])) == b"a_indices" => {
                names.insert(word(&ins, 1)?);
            }
            15 => {
                models.push(word(&ins, 1)?);
                let end = (3..n)
                    .find(|&j| ins.get(j).is_some_and(|w| w.to_le_bytes().contains(&0)))
                    .ok_or_else(|| fatal("StopIteration"))?;
                interface.extend(ins[(end + 1).min(ins.len())..].iter().copied());
            }
            21 => {
                let id = word(&ins, 1)?;
                let shape = ins[2.min(ins.len())..].to_vec();
                match ints.iter_mut().find(|(t, _)| *t == id) {
                    Some(e) => e.1 = shape,
                    None => ints.push((id, shape)),
                }
            }
            32 => {
                let id = word(&ins, 1)?;
                let shape = ins[2.min(ins.len())..].to_vec();
                match pointers.iter_mut().find(|(t, _)| *t == id) {
                    Some(e) => e.1 = shape,
                    None => pointers.push((id, shape)),
                }
            }
            59 => {
                variables.insert(word(&ins, 2)?, (word(&ins, 1)?, word(&ins, 3)?));
            }
            OP_DECORATE if n >= 4 && word(&ins, 2)? == 30 => {
                locations.insert(word(&ins, 1)?);
            }
            _ => {}
        }
        instructions.push(ins);
        i += n;
    }
    if models != [0] {
        return Ok(words);
    }
    let ptr = |id: u32| {
        pointers
            .iter()
            .find(|(t, _)| *t == id)
            .map(|(_, s)| s.clone())
    };
    let int = |id: u32| ints.iter().find(|(t, _)| *t == id).map(|(_, s)| s.clone());
    let mut targets: BTreeSet<u32> = BTreeSet::new();
    for &v in &names {
        if !(locations.contains(&v) && interface.contains(&v)) {
            continue;
        }
        let Some(&(pt, storage)) = variables.get(&v) else {
            continue;
        };
        if storage != 1 {
            continue;
        }
        let Some(p) = ptr(pt) else { continue };
        if p.first() != Some(&1) {
            continue;
        }
        let pointee = *p.get(1).ok_or_else(index_error)?;
        if int(pointee).as_deref() == Some(&[32, 1][..]) {
            targets.insert(v);
        }
    }
    if targets.is_empty() {
        return Ok(words);
    }
    let mut in_function = false;
    for ins in &instructions {
        let op = ins[0] & 0xffff;
        if op == 54 {
            in_function = true;
        }
        if op == 8 || op == 317 {
            continue;
        }
        if in_function
            && ins[1..].iter().any(|w| targets.contains(w))
            && !(op == 61 && targets.contains(&word(ins, 3)?))
        {
            return red("RenderDragon a_indices input has a use other than OpLoad");
        }
    }
    let loaded: HashSet<u32> = instructions
        .iter()
        .filter(|ins| ins[0] & 0xffff == 61)
        .filter_map(|ins| ins.get(3).copied())
        .collect();
    targets.retain(|t| loaded.contains(t));
    if targets.is_empty() {
        return Ok(words);
    }
    let mut bound = word(&words, 3)?;
    let uint = match ints.iter().find(|(_, s)| s[..] == [32, 0]) {
        Some((t, _)) => *t,
        None => {
            bound += 1;
            bound - 1
        }
    };
    let pointer = match pointers.iter().find(|(_, s)| s[..] == [1, uint]) {
        Some((t, _)) => *t,
        None => {
            bound += 1;
            bound - 1
        }
    };
    let mut out = words[..5].to_vec();
    let mut inserted = false;
    for ins in &instructions {
        let op = ins[0] & 0xffff;
        if op == 21 && !inserted {
            out.extend_from_slice(&[
                (4 << 16) | 21,
                uint,
                32,
                0,
                (4 << 16) | 32,
                pointer,
                1,
                uint,
            ]);
            inserted = true;
        }
        if (op == 21 || op == 32) && (ins[1] == uint || ins[1] == pointer) {
            continue;
        }
        let mut ins = ins.clone();
        if op == 59 && targets.contains(&word(&ins, 2)?) {
            ins = [vec![ins[0], pointer], ins[2..].to_vec()].concat();
        }
        if op == 61 && targets.contains(&word(&ins, 3)?) {
            out.extend_from_slice(&[ins[0], uint, bound]);
            out.extend_from_slice(&ins[3..]);
            out.extend_from_slice(&[(4 << 16) | 124, ins[1], ins[2], bound]);
            bound += 1;
        } else {
            out.extend_from_slice(&ins);
        }
    }
    out[3] = bound;
    Ok(out)
}

/// `spv_ops(words)`: `(opcode, operands)` of every instruction after the header
fn spv_ops(words: &[u32]) -> Result<Vec<(u32, Vec<u32>)>, Fatal> {
    let mut out = Vec::new();
    let mut i = 5;
    while i < words.len() {
        let (op, n) = (words[i] & 0xffff, (words[i] >> 16) as usize);
        if !(0 < n && n <= words.len() - i) {
            return red("malformed SPIR-V instruction length");
        }
        out.push((op, words[i + 1..i + n].to_vec()));
        i += n;
    }
    Ok(out)
}

/// `entry_interface(entry)`: the interface ids after the entry point's NUL-terminated name
fn entry_interface(entry: &[u32]) -> Result<&[u32], Fatal> {
    let i = (2..entry.len())
        .find(|&i| entry[i].to_le_bytes().contains(&0))
        .ok_or_else(|| fatal("StopIteration"))?;
    Ok(&entry[i + 1..])
}

fn one_entry(ops: &[(u32, Vec<u32>)]) -> Result<&[u32], Fatal> {
    let mut it = ops.iter().filter(|(op, _)| *op == 15);
    match (it.next(), it.next()) {
        (Some((_, a)), None) => Ok(a),
        _ => Err(fatal("ValueError: not exactly one OpEntryPoint")),
    }
}

/// A value of the symbolic run of [`geometry_viewport`]: Python ints / bools, tuples, and `object()`s compared by identity
#[derive(Clone, Debug)]
enum Val {
    Int(i64),
    Bool(bool),
    Sym(usize),
    Tup(Vec<Val>),
}

impl Val {
    /// a Python int (a bool is one)
    fn int(&self) -> Option<i64> {
        match self {
            Val::Int(i) => Some(*i),
            Val::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }
    /// `self[i]`
    fn at(&self, i: &Val) -> Option<Val> {
        match self {
            Val::Tup(t) => {
                let i = i.int()?;
                if i < 0 {
                    return None; // never: the indices are words
                }
                t.get(i as usize).cloned()
            }
            _ => None,
        }
    }
    /// `self is other` where `other` is an `object()`
    fn is(&self, other: &Val) -> bool {
        matches!((self, other), (Val::Sym(a), Val::Sym(b)) if a == b)
    }
}

/// `geometry_viewport(words)`: the input location a triangle pass-through geometry stage copies to ViewportIndex, when it provably emits the three input vertices
/// in order, forwarding every output; `None` otherwise
pub fn geometry_viewport(words: &[u32]) -> Result<Option<u32>, Fatal> {
    let ops = spv_ops(words)?;
    let entry = one_entry(&ops)?;
    let func = word(entry, 1)?;
    let modes: BTreeSet<Vec<u32>> = ops
        .iter()
        .filter(|(op, a)| *op == 16 && a.first() == Some(&func))
        .map(|(_, a)| a[1..].to_vec())
        .collect();
    let want: BTreeSet<Vec<u32>> = [vec![22], vec![29], vec![26, 3]].into_iter().collect();
    let got: BTreeSet<Vec<u32>> = modes.into_iter().filter(|m| m[..] != [0, 1]).collect();
    if got != want {
        return Ok(None);
    }
    let fns: Vec<u32> = ops
        .iter()
        .filter(|(op, _)| *op == 54)
        .map(|(_, a)| word(a, 1))
        .collect::<Result<_, _>>()?;
    if fns != [func] {
        return Ok(None);
    }
    if ops
        .iter()
        .any(|(op, a)| *op == 17 && !(a[..] == [1] || a[..] == [2] || a[..] == [57]))
    {
        return Ok(None);
    }
    if ops.iter().any(|(op, _)| matches!(op, 10 | 331 | 332)) {
        return Ok(None);
    }
    let mut dec: HashMap<u32, (u32, u32)> = HashMap::new();
    for (op, a) in &ops {
        if *op == OP_DECORATE {
            if a.len() != 3
                || !(a[1] == DEC_BUILTIN || a[1] == DEC_LOCATION)
                || dec.contains_key(&a[0])
            {
                return Ok(None);
            }
            dec.insert(a[0], (a[1], a[2]));
        }
    }
    let ints: HashSet<u32> = ops
        .iter()
        .filter(|(op, a)| *op == 21 && a.get(1) == Some(&32))
        .map(|(_, a)| a[0])
        .collect();
    let uints: HashSet<u32> = ops
        .iter()
        .filter(|(op, a)| *op == 21 && a[1..] == [32, 0])
        .map(|(_, a)| a[0])
        .collect();
    let mut scalars = ints.clone();
    scalars.extend(
        ops.iter()
            .filter(|(op, a)| *op == 22 && a.get(1) == Some(&32))
            .map(|(_, a)| a[0]),
    );
    let mut varyings = scalars.clone();
    varyings.extend(
        ops.iter()
            .filter(|(op, a)| *op == 23 && a.get(1).is_some_and(|t| scalars.contains(t)))
            .map(|(_, a)| a[0]),
    );
    let mut vals: HashMap<u32, Val> = HashMap::new();
    for (op, a) in &ops {
        if *op == 43 && a.first().is_some_and(|t| ints.contains(t)) {
            vals.insert(word(a, 1)?, Val::Int(word(a, 2)? as i64));
        }
    }
    let mut ptr_types: HashMap<u32, Vec<u32>> = HashMap::new();
    for (op, a) in &ops {
        if *op == 32 {
            ptr_types.insert(word(a, 0)?, a[1..].to_vec());
        }
    }
    let mut arrays: HashMap<u32, u32> = HashMap::new();
    for (op, a) in &ops {
        if *op == 28 && matches!(vals.get(&word(a, 2)?), Some(v) if v.int() == Some(3)) {
            arrays.insert(a[0], a[1]);
        }
    }
    // OpVariable outside Function, in the order of the module
    let mut variables: Vec<(u32, u32)> = Vec::new();
    for (op, a) in &ops {
        if *op == 59 && word(a, 2)? != 7 {
            let v = a[1];
            match variables.iter_mut().find(|(x, _)| *x == v) {
                Some(e) => e.1 = a[0],
                None => variables.push((v, a[0])),
            }
        }
    }
    let var_set: HashSet<u32> = variables.iter().map(|(v, _)| *v).collect();
    let iface: HashSet<u32> = entry_interface(entry)?.iter().copied().collect();
    if var_set != iface {
        return Ok(None);
    }
    let mut next_sym = 0usize;
    let mut sym = || {
        next_sym += 1;
        Val::Sym(next_sym)
    };
    type Key = (u32, u32);
    let mut inputs: Vec<(Key, (u32, u32))> = Vec::new(); // decoration -> (element type, variable)
    let mut outputs: Vec<(Key, (u32, u32))> = Vec::new();
    let mut memory: HashMap<u32, Val> = HashMap::new();
    for &(var, ty) in &variables {
        let Some(&d) = dec.get(&var) else {
            return Ok(None);
        };
        let pt = ptr_types.get(&ty).ok_or_else(|| fatal("KeyError"))?;
        if pt.len() != 2 {
            return Err(fatal("ValueError: not enough values to unpack"));
        }
        let (storage, ty) = (pt[0], pt[1]);
        if storage == 1 && arrays.get(&ty).is_some_and(|e| varyings.contains(e)) {
            if inputs.iter().any(|(k, _)| *k == d) {
                return Ok(None);
            }
            inputs.push((d, (arrays[&ty], var)));
            memory.insert(var, Val::Tup(vec![sym(), sym(), sym()]));
        } else if storage == 3 && varyings.contains(&ty) {
            if outputs.iter().any(|(k, _)| *k == d) {
                return Ok(None);
            }
            outputs.push((d, (ty, var)));
        } else {
            return Ok(None);
        }
    }
    let find = |m: &[(Key, (u32, u32))], k: Key| m.iter().find(|(x, _)| *x == k).map(|(_, v)| *v);
    let (position, viewport) = ((DEC_BUILTIN, POSITION), (DEC_BUILTIN, VIEWPORT_INDEX));
    if find(&outputs, position).is_none()
        || !find(&outputs, viewport).is_some_and(|(t, _)| uints.contains(&t))
    {
        return Ok(None);
    }
    let mut forwarded: Vec<(u32, Val)> = Vec::new();
    for &(d, (ty, var)) in &outputs {
        if d == viewport {
            continue;
        }
        if d != position && d.0 != DEC_LOCATION {
            return Ok(None);
        }
        match find(&inputs, d) {
            Some((ity, ivar)) if ity == ty => forwarded.push((var, memory[&ivar].clone())),
            _ => return Ok(None),
        }
    }
    let routes: Vec<(u32, Val)> = inputs
        .iter()
        .filter(|(d, (ty, _))| {
            d.0 == DEC_LOCATION && d.1 < 32 && find(&outputs, *d).is_none() && uints.contains(ty)
        })
        .map(|(d, (_, var))| (d.1, memory[var].clone()))
        .collect();
    if routes.is_empty() {
        return Ok(None);
    }
    let target = find(&outputs, viewport).unwrap().1;
    let first_fn = ops.iter().position(|(op, _)| *op == 54).unwrap();
    let body = &ops[first_fn + 1..];
    let mut labels: HashMap<u32, usize> = HashMap::new();
    for (i, (op, a)) in body.iter().enumerate() {
        if *op == 248
            && let Some(&l) = a.first()
        {
            labels.insert(l, i);
        }
    }
    let forwarded_ids: HashSet<u32> = forwarded.iter().map(|(v, _)| *v).collect();
    let mut st = Run {
        body,
        labels,
        vals,
        memory,
        pointers: variables
            .iter()
            .map(|(v, _)| (*v, (*v, Vec::new())))
            .collect(),
        locals: HashSet::new(),
        ints,
        forwarded,
        forwarded_ids,
        routes,
        target,
    };
    Ok(st.run())
}

/// The state of [`geometry_viewport`]'s symbolic run
struct Run<'a> {
    body: &'a [(u32, Vec<u32>)],
    labels: HashMap<u32, usize>,
    vals: HashMap<u32, Val>,
    memory: HashMap<u32, Val>,
    pointers: HashMap<u32, (u32, Vec<Val>)>,
    locals: HashSet<u32>,
    ints: HashSet<u32>,
    forwarded: Vec<(u32, Val)>,
    forwarded_ids: HashSet<u32>,
    routes: Vec<(u32, Val)>,
    target: u32,
}

impl Run<'_> {
    /// The `try:` block: `None` for a refusal and for every IndexError / KeyError / TypeError
    fn run(&mut self) -> Option<u32> {
        let (mut route, mut emitted, mut ended) = (None::<u32>, 0usize, false);
        let (mut pc, mut block): (usize, Option<u32>) = (0, None);
        let mut prev: Option<u32>;
        for _ in 0..256 {
            let (op, a) = self.body.get(pc)?;
            let a = a.as_slice();
            pc += 1;
            match *op {
                248 => {
                    prev = block;
                    block = Some(*a.first()?);
                    let mut phis = Vec::new();
                    while self.body.get(pc)?.0 == 245 {
                        let phi = &self.body[pc].1;
                        // dict(zip(parents, values))[prev]: the last pair for a parent wins
                        let mut pick = None;
                        let mut k = 2;
                        while k < phi.len() {
                            let (v, p) = (phi[k], phi.get(k + 1).copied());
                            if let Some(p) = p
                                && Some(p) == prev
                            {
                                pick = Some(v);
                            }
                            k += 2;
                        }
                        let v = self.vals.get(&pick?)?.clone();
                        phis.push((*phi.get(1)?, v));
                        pc += 1;
                    }
                    self.vals.extend(phis);
                }
                59 => {
                    if a.len() != 3 || a[2] != 7 {
                        return None;
                    }
                    self.locals.insert(a[1]);
                    self.pointers.insert(a[1], (a[1], Vec::new()));
                }
                65 => {
                    let (base, mut indices) = self.pointers.get(a.get(2)?)?.clone();
                    for i in a.get(3..)? {
                        indices.push(self.vals.get(i)?.clone());
                    }
                    self.pointers.insert(*a.get(1)?, (base, indices));
                }
                61 => {
                    if a.len() != 3 {
                        return None;
                    }
                    let (base, indices) = self.pointers.get(&a[2])?.clone();
                    let mut value = self.memory.get(&base)?.clone();
                    for i in &indices {
                        value = value.at(i)?;
                    }
                    self.vals.insert(a[1], value);
                }
                62 => {
                    if a.len() != 2
                        || (!self.locals.contains(&a[0])
                            && a[0] != self.target
                            && !self.forwarded_ids.contains(&a[0]))
                    {
                        return None;
                    }
                    let v = self.vals.get(&a[1])?.clone();
                    self.memory.insert(a[0], v);
                }
                80 => {
                    let mut t = Vec::new();
                    for i in a.get(2..)? {
                        t.push(self.vals.get(i)?.clone());
                    }
                    self.vals.insert(*a.get(1)?, Val::Tup(t));
                }
                81 => {
                    let mut value = self.vals.get(a.get(2)?)?.clone();
                    for &i in a.get(3..)? {
                        value = value.at(&Val::Int(i as i64))?;
                    }
                    self.vals.insert(a[1], value);
                }
                83 => {
                    let v = self.vals.get(a.get(2)?)?.clone();
                    self.vals.insert(*a.get(1)?, v);
                }
                128 if a.first().is_some_and(|t| self.ints.contains(t)) => {
                    let x = self.vals.get(a.get(2)?)?.int()?;
                    let y = self.vals.get(a.get(3)?)?.int()?;
                    self.vals.insert(a[1], Val::Int((x + y) & 0xffff_ffff));
                }
                177 => {
                    let x = self.vals.get(a.get(2)?)?.int()?;
                    let y = self.vals.get(a.get(3)?)?.int()?;
                    self.vals
                        .insert(*a.get(1)?, Val::Bool((x ^ 0x8000_0000) < (y ^ 0x8000_0000)));
                }
                218 => {
                    if ended || emitted == 3 {
                        return None;
                    }
                    for (var, values) in &self.forwarded {
                        let m = self.memory.remove(var)?;
                        if !m.is(&values.at(&Val::Int(emitted as i64))?) {
                            return None;
                        }
                    }
                    let value = self.memory.remove(&self.target)?;
                    let mut matched = None;
                    for (loc, values) in &self.routes {
                        if value.is(&values.at(&Val::Int(emitted as i64))?) {
                            matched = Some(*loc);
                            break;
                        }
                    }
                    let m = matched?;
                    if emitted != 0 && route != Some(m) {
                        return None;
                    }
                    route = Some(m);
                    emitted += 1;
                }
                219 => {
                    if emitted != 3 || ended {
                        return None;
                    }
                    ended = true;
                }
                246 | 247 => {}
                249 => pc = *self.labels.get(a.first()?)?,
                250 => match self.vals.get(a.first()?)? {
                    Val::Bool(b) => {
                        pc = *self.labels.get(if *b { a.get(1)? } else { a.get(2)? })?
                    }
                    _ => return None,
                },
                253 => return if emitted == 3 { route } else { None },
                _ => return None,
            }
        }
        None
    }
}

/// `instance_locations(words)`: the scalar output locations a vertex shader stores only gl_InstanceIndex loads into (Metal needs one viewport index per primitive)
pub fn instance_locations(words: &[u32]) -> Result<Vec<u32>, Fatal> {
    let ops = spv_ops(words)?;
    let entry = one_entry(&ops)?;
    if ops.iter().filter(|(op, _)| *op == 54).count() != 1
        || ops.iter().any(|(op, a)| {
            matches!(op, 10 | 67 | 70)
                || (*op == 14 && a.first() != Some(&0))
                || (*op == 17 && a.first().is_some_and(|c| *c == 4441 || *c == 4442))
        })
    {
        return Ok(Vec::new());
    }
    let interface: HashSet<u32> = entry_interface(entry)?.iter().copied().collect();
    let builtin: HashSet<u32> = ops
        .iter()
        .filter(|(op, a)| {
            *op == OP_DECORATE
                && a.get(1..) == Some(&[DEC_BUILTIN, INSTANCE_INDEX][..])
                && interface.contains(&a[0])
        })
        .map(|(_, a)| a[0])
        .collect();
    let values: HashSet<u32> = ops
        .iter()
        .filter(|(op, a)| *op == 61 && a.get(2).is_some_and(|p| builtin.contains(p)))
        .map(|(_, a)| a[1])
        .collect();
    if values.is_empty() {
        return Ok(Vec::new());
    }
    let mut outputs: HashSet<u32> = ops
        .iter()
        .filter(|(op, a)| *op == 59 && a.get(2) == Some(&3) && interface.contains(&a[1]))
        .map(|(_, a)| a[1])
        .collect();
    let mut stores: HashMap<u32, HashSet<u32>> =
        outputs.iter().map(|v| (*v, HashSet::new())).collect();
    for (op, a) in &ops {
        if *op == 62 && a.first().is_some_and(|p| stores.contains_key(p)) {
            let v = *a.get(1).ok_or_else(index_error)?;
            stores.get_mut(&a[0]).unwrap().insert(v);
        } else if matches!(op, 65 | 66 | 83) && a.get(2).is_some_and(|p| stores.contains_key(p)) {
            outputs.remove(&a[2]);
        } else if matches!(op, 63 | 64) && a.first().is_some_and(|p| stores.contains_key(p)) {
            outputs.remove(&a[0]);
        }
    }
    let mut locations: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (op, a) in &ops {
        if *op == OP_DECORATE && a.len() == 3 && a[1] == DEC_LOCATION && stores.contains_key(&a[0])
        {
            locations.entry(a[2]).or_default().push(a[0]);
        }
    }
    Ok(locations
        .into_iter()
        .filter(|(_, m)| {
            m.len() == 1
                && outputs.contains(&m[0])
                && !stores[&m[0]].is_empty()
                && stores[&m[0]].is_subset(&values)
        })
        .map(|(l, _)| l)
        .collect())
}

/// `viewport_variant(src, key, location)`: the vertex shader with output `location` as `[[viewport_array_index]]`, as `vk_<key>_vp<location>`
fn viewport_variant(src: &str, key: &str, location: u32) -> Option<String> {
    let out = Regex::new(&format!(r"(?s)struct vk_{key}_out\s*\{{(.*?)\}};")).unwrap();
    let c = out.captures(src)?;
    let body = c.get(1).unwrap();
    static ATTR: OnceLock<Regex> = OnceLock::new();
    if rx(&ATTR, r"\[\[([^\]]+)\]\]")
        .captures_iter(body.as_str())
        .any(|a| &a[1] != "position" && !a[1].starts_with("user("))
    {
        return None;
    }
    let mem = Regex::new(&format!(
        r"\buint\s+\w+\s+(\[\[user\(locn{location}\)\]\]);"
    ))
    .unwrap();
    let m = mem.captures(body.as_str())?.get(1).unwrap();
    let (i, j) = (body.start() + m.start(), body.start() + m.end());
    let v = format!("{}[[viewport_array_index]]{}", &src[..i], &src[j..]);
    Some(v.replace(&format!("vk_{key}"), &format!("vk_{key}_vp{location}")))
}

/// `entry_args(src, key, path)`: the span of the translated entry point's argument list
fn entry_args(src: &str, key: &str, path: &str) -> Result<(usize, usize), Fatal> {
    let r = Regex::new(&format!(r"(?s)\bvk_{key}\((.*?)\)\s*\{{")).unwrap();
    match r.captures(src) {
        Some(c) => {
            let g = c.get(1).unwrap();
            Ok((g.start(), g.end()))
        }
        None => red(format!(
            "{path}: entry point vk_{key} not found in the translated MSL"
        )),
    }
}

/// `param(pat, slot)`: move the entry-point argument `<pat> [[buffer(n)]]` to `slot`
fn param(
    src: &mut String,
    m: &mut (usize, usize),
    pat: &str,
    slot: Option<usize>,
    key: &str,
    path: &str,
) -> Result<bool, Fatal> {
    let r = Regex::new(&format!(r"{pat}\s+\[\[buffer\((\d+)\)\]\]")).unwrap();
    let args = &src[m.0..m.1];
    let Some(c) = r.captures(args) else {
        return Ok(false);
    };
    let g = c.get(1).unwrap();
    let (s0, s1) = (m.0 + g.start(), m.0 + g.end());
    let slot = slot.map(|s| s.to_string()).unwrap_or_else(|| "None".into());
    *src = format!("{}{}{}", &src[..s0], slot, &src[s1..]);
    *m = entry_args(src, key, path)?;
    Ok(true)
}

/// A small insertion-ordered map (the Python dicts whose order reaches the output)
struct OMap<K, V>(Vec<(K, V)>);

impl<K: PartialEq + Copy, V> OMap<K, V> {
    fn new() -> Self {
        OMap(Vec::new())
    }
    fn get(&self, k: &K) -> Option<&V> {
        self.0.iter().find(|(x, _)| x == k).map(|(_, v)| v)
    }
    fn insert(&mut self, k: K, v: V) {
        match self.0.iter_mut().find(|(x, _)| *x == k) {
            Some(e) => e.1 = v,
            None => self.0.push((k, v)),
        }
    }
    fn entry(&mut self, k: K, v: impl FnOnce() -> V) -> &mut V {
        let i = match self.0.iter().position(|(x, _)| *x == k) {
            Some(i) => i,
            None => {
                self.0.push((k, v()));
                self.0.len() - 1
            }
        };
        &mut self.0[i].1
    }
}

/// What [`one`] returns for a shader
enum Row {
    /// `(key, stage field, resources, [(msl file name, text)], where)`
    Ok(String, String, Vec<String>, Vec<(String, String)>, String),
    /// a stage that is skipped and counted
    Skipped(String),
}

fn jint(v: &Value) -> Option<i64> {
    v.as_i64()
}

/// `one(source, tmp)`
fn one(source: &Source, tools: &dyn LayerTools, tmp: &Path) -> Result<Row, Fatal> {
    let (key, path, data) = source;
    if !data.len().is_multiple_of(4) {
        return red(format!(
            "{path}: SPIR-V length {} is not a multiple of 4",
            data.len()
        ));
    }
    let mut words = fix_strides(crate::spv::words(data))?;
    if path.contains(".material.bin@") {
        words = unsigned_bone_indices(words)?;
    }
    let src_spv = tmp.join(format!("{key}.in.spv"));
    std::fs::write(&src_spv, crate::spv::bytes(&words)).map_err(fatal)?;
    let src_spv_s = src_spv.to_string_lossy().into_owned();
    let r = run(tools, Tool::SpirvCross, &[&src_spv_s, "--reflect"]);
    if r.rc != 0 {
        return Err(fatal(format!(
            "subprocess.CalledProcessError: spirv-cross {src_spv_s} --reflect returned non-zero exit status {}",
            r.rc
        )));
    }
    let refl: Value =
        serde_json::from_str(&r.stdout).map_err(|e| fatal(format!("json.JSONDecodeError: {e}")))?;
    let empty = Vec::new();
    let eps = refl
        .get("entryPoints")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    if eps.len() != 1 {
        return red(format!(
            "{path}: entry points {} are not exactly one",
            py_repr(refl.get("entryPoints").unwrap_or(&Value::Array(vec![])))
        ));
    }
    let ep = &eps[0];
    let stage = ep
        .get("mode")
        .and_then(|v| v.as_str())
        .ok_or_else(|| fatal("KeyError: 'mode'"))?
        .to_string();
    if stage == "geom"
        && let Some(location) = geometry_viewport(&words)?
    {
        return Ok(Row::Ok(
            key.clone(),
            format!("geomvp:{location}"),
            vec![],
            vec![],
            path.clone(),
        ));
    }
    if !matches!(stage.as_str(), "vert" | "frag" | "comp") {
        return Ok(Row::Skipped(stage));
    }
    let mut field = stage.clone();
    if stage == "comp" {
        let wg: Vec<Value> = ep
            .get("workgroup_size")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let spec = ep
            .get("workgroup_size_is_spec_constant_id")
            .and_then(|v| v.as_array())
            .is_some_and(|a| a.iter().any(truthy));
        if wg.len() != 3 || spec {
            return Ok(Row::Skipped(
                "comp with a local size from specialization constants".into(),
            ));
        }
        field = format!(
            "comp:{}",
            wg.iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    let dec = decorations(&words)?;
    type Pair = (i64, i64);
    let mut by_pair: OMap<Pair, char> = OMap::new();
    let mut count: BTreeMap<Pair, i64> = BTreeMap::new();
    for (k, kind) in KINDS {
        for r in refl.get(k).and_then(|v| v.as_array()).unwrap_or(&empty) {
            let set = r.get("set").and_then(jint).unwrap_or(0);
            let binding = r
                .get("binding")
                .and_then(jint)
                .ok_or_else(|| fatal("KeyError: 'binding'"))?;
            let pair = (set, binding);
            by_pair.insert(pair, kind);
            let dims: Vec<Value> = match r.get("array").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a.clone(),
                _ => vec![Value::from(1)],
            };
            if dims.len() != 1 || !truthy(&dims[0]) {
                return red(format!(
                    "{path}: array dimensions {} of resource {} are not implemented",
                    py_repr(&Value::Array(dims.clone())),
                    r.get("name")
                        .map(|n| n
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| py_repr(n)))
                        .unwrap_or_else(|| "None".into())
                ));
            }
            count.insert(pair, dims[0].as_i64().unwrap_or(0));
        }
    }
    let pcs = refl
        .get("push_constants")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    if pcs.len() > 1 {
        return red(format!("{path}: {} push constant blocks", pcs.len()));
    }
    // sets stay discrete until their combined per-stage bindings exceed Metal's limits
    let mut set_counts: OMap<i64, Counts> = OMap::new();
    let dynamic_sets: HashSet<i64> = by_pair
        .0
        .iter()
        .filter(|(_, k)| matches!(k, 'u' | 'b'))
        .map(|(p, _)| p.0)
        .collect();
    for (pair, kind) in &by_pair.0 {
        let n = count[pair];
        let per = set_counts.entry(pair.0, Counts::default);
        if matches!(kind, 'u' | 'b') {
            per.add(Class::Buffer, n);
        }
        if matches!(kind, 't' | 'c') {
            per.add(Class::Texture, n);
        }
        if matches!(kind, 's' | 'c') {
            per.add(Class::Sampler, n);
        }
    }
    let mut argsets: Vec<i64> = Vec::new();
    let binding_totals = |argsets: &[i64]| {
        let mut t = Counts::default();
        t.add(Class::Buffer, !pcs.is_empty() as i64);
        for (pair, kind) in &by_pair.0 {
            if argsets.contains(&pair.0) {
                continue;
            }
            let n = count[pair];
            if matches!(kind, 'u' | 'b') {
                t.add(Class::Buffer, n);
            }
            if matches!(kind, 't' | 'c') {
                t.add(Class::Texture, n);
            }
            if matches!(kind, 's' | 'c') {
                t.add(Class::Sampler, n);
            }
        }
        t.add(
            Class::Buffer,
            argsets.len() as i64 + argsets.iter().any(|s| dynamic_sets.contains(s)) as i64,
        );
        t
    };
    loop {
        let totals = binding_totals(&argsets);
        let over: Vec<Class> = LIMITS
            .iter()
            .filter(|(c, l)| totals.get(*c) > *l as i64)
            .map(|(c, _)| *c)
            .collect();
        if over.is_empty() {
            break;
        }
        let mut choices: Vec<(i64, i64)> = Vec::new();
        for (s, counts) in &set_counts.0 {
            if argsets.contains(s) {
                continue;
            }
            let mut gains: Vec<(Class, i64)> = over
                .iter()
                .map(|c| (*c, counts.get(*c) - if *c == Class::Buffer { 1 } else { 0 }))
                .collect();
            if over.contains(&Class::Buffer)
                && dynamic_sets.contains(s)
                && !argsets.iter().any(|t| dynamic_sets.contains(t))
            {
                for g in gains.iter_mut().filter(|g| g.0 == Class::Buffer) {
                    g.1 -= 1;
                }
            }
            let score: i64 = gains.iter().map(|(_, n)| (*n).max(0)).sum();
            if score != 0 {
                choices.push((score, *s));
            }
        }
        if choices.is_empty() {
            return red(format!("{path}: resource slots exceeded {}", totals.repr()));
        }
        // max(choices, key=(score, -set)): the first of equal keys, and the keys are unique
        let best = choices
            .iter()
            .copied()
            .fold(None::<(i64, i64)>, |b, c| match b {
                Some(b) if (b.0, -b.1) >= (c.0, -c.1) => Some(b),
                _ => Some(c),
            });
        argsets.push(best.unwrap().1);
    }
    argsets.sort();
    // a Vulkan descriptor array takes one binding; each element of it its own argument-buffer ID
    let mut arg_ids: Vec<(Pair, i64)> = Vec::new();
    let mut next_id: BTreeMap<i64, i64> = argsets.iter().map(|s| (*s, 0)).collect();
    for (pair, n) in &count {
        if let Some(id) = next_id.get_mut(&pair.0) {
            arg_ids.push((*pair, *id));
            *id += n;
        }
    }
    let arg_id = |p: &Pair| arg_ids.iter().find(|(x, _)| x == p).map(|(_, i)| *i);
    let mut slots: HashMap<Pair, i64> = HashMap::new();
    let mut nxt = Counts::default();
    let mut res: Vec<String> = Vec::new();
    let mut order: Vec<&(u32, Dec)> = dec.iter().collect();
    order.sort_by_key(|(_, d)| (d.set.unwrap_or(0), d.binding.unwrap_or(0))); // stable, as `sorted`
    for (_, d) in order {
        let Some(at) = d.binding_at else { continue };
        let pair = (d.set.unwrap_or(0) as i64, d.binding.unwrap() as i64);
        let Some(&kind) = by_pair.get(&pair) else {
            continue; // declared but unused: spirv-cross drops it
        };
        if let Some(id) = arg_id(&pair) {
            words[at] = id as u32;
            continue;
        }
        let n = match slots.get(&pair) {
            Some(n) => *n,
            None => {
                let (cls, k) = (slot_class(kind).unwrap(), count[&pair]);
                let n = nxt.get(cls);
                nxt.add(cls, k);
                if kind == 'c' {
                    let s = nxt.get(Class::Sampler).max(n + k);
                    nxt.0[Class::Sampler as usize] = s;
                }
                slots.insert(pair, n);
                res.push(format!(
                    "{kind}:{}.{}={n}{}",
                    pair.0,
                    pair.1,
                    if k > 1 {
                        format!("*{k}")
                    } else {
                        String::new()
                    }
                ));
                n
            }
        };
        words[at] = n as u32;
    }
    let pc_slot = if pcs.is_empty() {
        None
    } else {
        Some(nxt.get(Class::Buffer) as usize)
    };
    if !pcs.is_empty() {
        nxt.add(Class::Buffer, 1);
    }
    let mut ab_slot: BTreeMap<i64, usize> = BTreeMap::new();
    for s in &argsets {
        ab_slot.insert(*s, nxt.get(Class::Buffer) as usize);
        nxt.add(Class::Buffer, 1);
    }
    let dynb: Vec<Pair> = count
        .keys()
        .filter(|p| argsets.contains(&p.0) && matches!(by_pair.get(p), Some('u' | 'b')))
        .copied()
        .collect();
    let mut dyn_slot = None;
    if !dynb.is_empty() {
        dyn_slot = Some(nxt.get(Class::Buffer) as usize);
        nxt.add(Class::Buffer, 1);
    }
    if LIMITS.iter().any(|(c, l)| nxt.get(*c) > *l as i64) {
        return red(format!("{path}: resource slots exceeded {}", nxt.repr()));
    }
    let spv = tmp.join(format!("{key}.spv"));
    let msl = tmp.join(format!("{key}.metal"));
    std::fs::write(&spv, crate::spv::bytes(&words)).map_err(fatal)?;
    let (spv_s, msl_s) = (
        spv.to_string_lossy().into_owned(),
        msl.to_string_lossy().into_owned(),
    );
    let ep_name = ep
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| fatal("KeyError: 'name'"))?
        .to_string();
    let vk_name = format!("vk_{key}");
    let mut cmd: Vec<String> = [
        spv_s.as_str(),
        "--msl",
        "--msl-version",
        "20300",
        "--msl-decoration-binding",
        "--msl-texture-1d-as-2d",
        "--msl-texture-buffer-native",
        "--rename-entry-point",
        &ep_name,
        &vk_name,
        &stage,
        "--output",
        &msl_s,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if !argsets.is_empty() {
        cmd.extend(["--msl-argument-buffers", "--msl-argument-buffer-tier", "1"].map(String::from));
        for s in 0..8 {
            if !argsets.contains(&s) {
                cmd.extend(["--msl-discrete-descriptor-set".to_string(), s.to_string()]);
            }
        }
        for p in &dynb {
            cmd.extend([
                "--msl-dynamic-buffer".to_string(),
                p.0.to_string(),
                arg_id(p).unwrap().to_string(),
            ]);
        }
    }
    if stage == "vert" {
        cmd.push("--flip-vert-y".into());
    }
    let argv: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();
    let r = run(tools, Tool::SpirvCross, &argv);
    if r.rc != 0 {
        return red(format!(
            "{path}: spirv-cross failed: {}",
            head_chars(crate::py::strip(&r.stderr), 300)
        ));
    }
    let raw = std::fs::read(&msl)
        .map_err(|e| fatal(format!("FileNotFoundError: {}: {e}", msl.display())))?;
    let mut src = crate::py::text_mode(&String::from_utf8_lossy(&raw));
    let mut m = entry_args(&src, key, path)?;
    if let Some(pc) = pcs.first() {
        // the push-constant argument `constant <block type>& name [[buffer(0)]]` goes to its own slot
        let ty = pc
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| fatal("KeyError: 'type'"))?;
        let tname = refl
            .get("types")
            .and_then(|t| t.get(ty))
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| fatal("KeyError"))?;
        static SAN: OnceLock<Regex> = OnceLock::new();
        let tname = rx(&SAN, r"[^A-Za-z0-9_]")
            .replace_all(tname, "_")
            .into_owned();
        let r = Regex::new(&format!(
            r"constant\s+{}&\s+\w+\s+\[\[buffer\((\d+)\)\]\]",
            regex::escape(&tname)
        ))
        .unwrap();
        let args = src[m.0..m.1].to_string();
        if let Some(c) = r.captures(&args) {
            let g = c.get(1).unwrap();
            let (s0, s1) = (m.0 + g.start(), m.0 + g.end());
            src = format!("{}{}{}", &src[..s0], pc_slot.unwrap(), &src[s1..]);
            m = entry_args(&src, key, path)?;
            res.push(format!("p:0.0={}", pc_slot.unwrap()));
        }
    }
    if !argsets.is_empty() {
        // (set, member name) -> argument-buffer ID
        let mut names: OMap<(i64, &str), i64> = OMap::new();
        static SETS: OnceLock<Regex> = OnceLock::new();
        static IDS: OnceLock<Regex> = OnceLock::new();
        let sets = rx(
            &SETS,
            r"(?s)struct spvDescriptorSetBuffer(\d+)\s*\{(.*?)\};",
        );
        let ids_rx = rx(&IDS, r"(\w+)\s+\[\[id\((\d+)\)\]\]");
        let snapshot = src.clone();
        for c in sets.captures_iter(&snapshot) {
            let s: i64 = c[1].parse().map_err(fatal)?;
            for d in ids_rx.captures_iter(c.get(2).unwrap().as_str()) {
                names.insert(
                    (s, d.get(1).unwrap().as_str()),
                    d[2].parse().map_err(fatal)?,
                );
            }
        }
        for s in &argsets {
            if !param(
                &mut src,
                &mut m,
                &format!("spvDescriptorSet{s}"),
                Some(ab_slot[s]),
                key,
                path,
            )? {
                continue; // the shader uses nothing of the set
            }
            let ids: HashSet<i64> = names
                .0
                .iter()
                .filter(|((t, _), _)| t == s)
                .map(|(_, i)| *i)
                .collect();
            for (p, i) in &arg_ids {
                if p.0 != *s || !ids.contains(i) {
                    continue;
                }
                let kind = *by_pair.get(p).unwrap();
                if kind == 'c' {
                    return red(format!(
                        "{path}: combined image sampler ({}, {}) in an argument buffer not implemented",
                        p.0, p.1
                    ));
                }
                let array = if count[p] > 1 {
                    format!("*{}", count[p])
                } else {
                    String::new()
                };
                res.push(format!(
                    "{}:{s}.{}={}{array}@{i}",
                    kind.to_ascii_uppercase(),
                    p.1,
                    ab_slot[s]
                ));
            }
        }
        if param(&mut src, &mut m, "spvDynamicOffsets", dyn_slot, key, path)? {
            res.push(format!(
                "q:0.0={}",
                dyn_slot
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "None".into())
            ));
            static DYN: OnceLock<Regex> = OnceLock::new();
            let d = rx(
                &DYN,
                r"spvDescriptorSet(\d+)\.(\w+)\s*\+\s*spvDynamicOffsets\[(\d+)\]",
            );
            let found: BTreeSet<(String, String, String)> = d
                .captures_iter(&src)
                .map(|c| (c[1].to_string(), c[2].to_string(), c[3].to_string()))
                .collect();
            for (s, name, i) in found {
                let si: i64 = s.parse().map_err(fatal)?;
                let id = *names
                    .get(&(si, name.as_str()))
                    .ok_or_else(|| fatal(format!("KeyError: ({si}, '{name}')")))?;
                let b = arg_ids
                    .iter()
                    .find(|((t, _), x)| *t == si && *x == id)
                    .map(|((_, b), _)| *b)
                    .ok_or_else(|| fatal(format!("KeyError: ({si}, {id})")))?;
                res.push(format!("o:{s}.{b}={i}"));
            }
        }
    }
    static USED: OnceLock<Regex> = OnceLock::new();
    let used: HashSet<(Class, i64)> = rx(&USED, r"\[\[(buffer|texture|sampler)\((\d+)\)\]\]")
        .captures_iter(&src[m.0..m.1])
        .filter_map(|c| Some((Class::of(&c[1])?, c[2].parse().ok()?)))
        .collect();
    let mut kept = Vec::new();
    for x in res {
        let first = x.chars().next().unwrap();
        match slot_class(first) {
            None => kept.push(x),
            Some(cls) => {
                let n: i64 = x
                    .rsplit('=')
                    .next()
                    .unwrap()
                    .split('*')
                    .next()
                    .unwrap()
                    .parse()
                    .map_err(fatal)?;
                if used.contains(&(cls, n)) {
                    kept.push(x);
                }
            }
        }
    }
    let res = kept;
    static PRECISE: OnceLock<Regex> = OnceLock::new();
    let src = rx(&PRECISE, r"\bfast::(min|max|clamp)\(")
        .replace_all(&src, "precise::${1}(")
        .into_owned();
    let mut msls = vec![(format!("{key}.metal"), src.clone())];
    if stage == "frag" {
        // Metal needs two outputs of the same type for the dual-source variant
        static COLOR: OnceLock<Regex> = OnceLock::new();
        let mut outs: BTreeMap<i64, String> = BTreeMap::new();
        for c in rx(&COLOR, r"(\w+)\s+\w+\s+\[\[color\((\d+)\)\]\]").captures_iter(&src) {
            outs.insert(c[2].parse().map_err(fatal)?, c[1].to_string());
        }
        if outs.keys().copied().collect::<Vec<_>>() == [0, 1] && outs[&0] == outs[&1] {
            let d = src
                .replace(&format!("vk_{key}"), &format!("vk_{key}_d"))
                .replace("[[color(1)]]", "[[color(0), index(1)]]");
            msls.push((format!("{key}_d.metal"), d));
        }
    } else if stage == "vert" {
        for location in instance_locations(&words)? {
            if let Some(v) = viewport_variant(&src, key, location) {
                msls.push((format!("{key}_vp{location}.metal"), v));
            }
        }
    }
    Ok(Row::Ok(key.clone(), field, res, msls, path.clone()))
}

/// Python truthiness of a JSON value
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `vk_shaders.py <roots>... <out>`: every SPIR-V module of the roots, translated. `work`: a scratch folder (the Python's `tempfile.TemporaryDirectory()`); `jobs`: threads.
pub fn build(
    roots: &[PathBuf],
    tools: &dyn LayerTools,
    work: &Path,
    jobs: usize,
) -> Result<Layer, Fatal> {
    let files = spirv_sources(roots)?;
    if files.is_empty() {
        let names: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
        return red(format!("no SPIR-V files in {}", names.join(" ")));
    }
    std::fs::create_dir_all(work).map_err(fatal)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs.max(1))
        .build()
        .map_err(fatal)?;
    let rows: Vec<Result<Row, Fatal>> = pool.install(|| {
        use rayon::prelude::*;
        files.par_iter().map(|s| one(s, tools, work)).collect()
    });
    let mut seen: Vec<Done> = Vec::new();
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows {
        match row? {
            Row::Skipped(s) => *skipped.entry(s).or_insert(0) += 1,
            Row::Ok(key, stage, res, msl, src) => match seen.iter_mut().find(|r| r.0 == key) {
                Some(r) => *r = (key, stage, res, msl, src),
                None => seen.push((key, stage, res, msl, src)),
            },
        }
    }
    let mut log = Vec::new();
    if !skipped.is_empty() {
        log.push(format!(
            "[RED] skipped unsupported shader stages: {}",
            skipped
                .iter()
                .map(|(k, v)| format!("{k} {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let suffixes: HashSet<String> = seen
        .iter()
        .filter(|r| r.1.starts_with("geomvp:"))
        .map(|r| format!("_vp{}.metal", r.1.split(':').nth(1).unwrap_or("")))
        .collect();
    let msl: Vec<(String, String)> = seen
        .iter()
        .flat_map(|r| r.3.iter())
        .filter(|(name, _)| {
            !name.contains("_vp") || suffixes.iter().any(|s| name.ends_with(s.as_str()))
        })
        .cloned()
        .collect();
    let rev = run(tools, Tool::SpirvCross, &["--revision"]);
    let both = format!("{}{}", rev.stdout, rev.stderr);
    let mut tsv = vec![
        HEADER.to_string(),
        format!("# spirv-cross {}", py_split(&both).join(" ")),
    ];
    let mut rows: Vec<&Done> = seen.iter().collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    for (k, stage, res, _, src) in rows {
        tsv.push(format!("{k}\t{stage}\t{}\t{src}", res.join(" ")));
    }
    log.push(format!("vk: {} shaders", seen.len()));
    Ok(Layer {
        api: Api::Vk,
        table: tsv.join("\n") + "\n",
        msl,
        log,
        shaders: seen.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Vec<u32> {
        vec![MAGIC, 0x0001_0000, 0, 100, 0]
    }

    #[test]
    fn a_member_stride_moves_onto_its_array_type() {
        let mut w = header();
        // OpMemberDecorate %struct 0 ArrayStride 16; OpTypeArray %arr %f %n; OpTypeStruct %struct %arr
        w.extend([(5 << 16) | OP_MEMBER_DECORATE, 10, 0, DEC_ARRAY_STRIDE, 16]);
        w.extend([(4 << 16) | OP_TYPE_ARRAY, 11, 2, 3]);
        w.extend([(3 << 16) | OP_TYPE_STRUCT, 10, 11]);
        let out = fix_strides(w.clone()).unwrap();
        assert_eq!(
            out[10..14],
            [(4 << 16) | OP_DECORATE, 11, DEC_ARRAY_STRIDE, 16]
        );
        assert_eq!(out.len(), w.len() + 4);
        // a stride already on the type: nothing changes
        let mut w2 = w.clone();
        w2.extend([(4 << 16) | OP_DECORATE, 11, DEC_ARRAY_STRIDE, 16]);
        assert_eq!(fix_strides(w2.clone()).unwrap(), w2);
    }

    #[test]
    fn a_zero_length_instruction_is_red() {
        let mut w = header();
        w.push(0);
        assert!(
            fix_strides(w.clone())
                .unwrap_err()
                .0
                .contains("instruction length 0")
        );
        assert!(decorations(&w).is_err());
    }

    #[test]
    fn decorations_keep_the_binding_word() {
        let mut w = header();
        w.extend([(4 << 16) | OP_DECORATE, 7, DEC_SET, 2]);
        w.extend([(4 << 16) | OP_DECORATE, 7, DEC_BINDING, 5]);
        w.extend([(4 << 16) | OP_DECORATE, 8, DEC_BINDING, 1]);
        let d = decorations(&w).unwrap();
        assert_eq!(d[0].0, 7);
        assert_eq!(
            (d[0].1.set, d[0].1.binding, d[0].1.binding_at),
            (Some(2), Some(5), Some(12))
        );
        assert_eq!(d[1].0, 8);
    }

    #[test]
    fn geometry_refusals() {
        // no execution modes: refused
        let mut w = header();
        let name = crate::spv::string_words(b"main");
        let mut ep = vec![3, 1];
        ep.extend(&name);
        w.push(((ep.len() + 1) << 16) as u32 | 15);
        w.extend(&ep);
        assert_eq!(geometry_viewport(&w).unwrap(), None);
        // two entry points: the Python's unpacking fails
        let mut w2 = w.clone();
        w2.push(((ep.len() + 1) << 16) as u32 | 15);
        w2.extend(&ep);
        assert!(geometry_viewport(&w2).is_err());
    }

    #[test]
    fn whole_module_and_bgfx() {
        let mut spv = header();
        spv.extend([(2 << 16) | 17, 1]); // OpCapability Shader
        let b = crate::spv::bytes(&spv);
        assert!(whole_module(&b, 0, b.len()));
        assert!(!whole_module(&b, 0, b.len() - 4));
        let mut blob = b"xxVSH\x05".to_vec();
        blob.extend([1, 2, 3, 4]); // hash
        blob.extend([0, 0]); // no uniforms
        blob.extend((b.len() as u32).to_le_bytes());
        let at = blob.len();
        blob.extend(&b);
        let m = bgfx_modules(&blob);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, at);
        assert_eq!(m[0].1, b);
    }

    #[test]
    fn python_reprs() {
        let v: Value =
            serde_json::from_str(r#"[{"name":"main","mode":"vert"},{"a":[1,true,null]}]"#).unwrap();
        assert_eq!(
            py_repr(&v),
            "[{'name': 'main', 'mode': 'vert'}, {'a': [1, True, None]}]"
        );
        assert_eq!(
            Counts([1, 2, 3]).repr(),
            "{'buffer': 1, 'texture': 2, 'sampler': 3}"
        );
        assert_eq!(py_split(" a\t b\n"), vec!["a", "b"]);
    }
}
