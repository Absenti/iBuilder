//! `dmsh_meshstage.py`: NVN2 (Switch 2) task and mesh shaders as Metal object and mesh functions.
//!
//! The translator reads such a shader as a compute kernel with its SPH taken off and its stage memory in shared memory (job `stage_memory`); `to_stage` wraps that kernel into the
//! Metal stage function. `info` / `memory` / `own_shared` are what `job_for` calls (the build driver's mesh branch), `to_stage` / `pixel_reads` what `finish` calls.
//!
//! A payload past Metal's (`MESH_PAYLOAD_MAX`, 16 KB) goes through a device buffer the host binds at `MESH_SPILL_SLOT` in both stages (`payload_spill`): word 0 its slice count, then
//! a slice of the payload's words (rounded to 4) for each of the draw's task workgroups. The task stores its payload in its threadgroup's slice behind a device-scope fence and hands
//! its mesh threadgroups the slice's word offset in Metal's payload; the mesh shader's input is then a shared memory of its own (job `stage_input_words`, the translator's
//! `dm_stage_in`), read from the slice instead of sitting in the mesh shader's stage memory. A task's stage memory has no input area either: its outputs follow word 0.
//!
//! Python `ValueError`s are `Err(message)` with the Python's text; the places the Python would raise another exception (an `IndexError` on a short SPH, a `KeyError` on an
//! information dict of a kind that is neither `ts` nor `ms`) are `Err` with a message that says so (the Python's build stops there).
use super::abi;
use super::pycompat::strip;
use regex::Regex;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::LazyLock;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

pub const WORKGROUP: i64 = 0;
pub const INPUT: i64 = 0x10;
/// vertices and primitives of an attribute block
pub const BLOCK: i64 = 32;
pub const METAL_MAX_VERTICES: i64 = 256;
pub const METAL_MAX_PRIMITIVES: i64 = 512;
/// mesh threadgroups an object threadgroup may launch
pub const METAL_MAX_GRID: i64 = abi::MESH_GRID;
/// threadgroup memory of a Metal function
pub const METAL_MAX_SHARED: i64 = 0x8000;
/// bytes of stage memory from which a mesh shader's array is a device slice
pub const SPILL_MIN: i64 = 6000;
/// `abi.MESH_PAYLOAD_MAX` (`DMSH_MESH_PAYLOAD_MAX`): Metal's limit of an object function's payload; a larger payload goes through a device buffer (`payload_spill`)
pub const MESH_PAYLOAD_MAX: i64 = abi::MESH_PAYLOAD_MAX;
/// `abi.MESH_SPILL_SLOT` (`DMSH_MESH_SPILL_SLOT`): the payload spill buffer's index in the object and mesh tables
pub const MESH_SPILL_SLOT: i64 = abi::MESH_SPILL_SLOT;

/// `payload_spill(i)`: whether a task or mesh shader's payload exceeds Metal's object payload limit and goes through a device buffer
pub fn payload_spill(i: &MeshInfo) -> bool {
    i.payload > MESH_PAYLOAD_MAX
}

/// `payload_words(i)`: words of one task workgroup's slice in the payload spill buffer (16-byte aligned)
pub fn payload_words(i: &MeshInfo) -> i64 {
    ((i.payload + 15) & !15) >> 2
}

/// the key of an SPH output component: the point size, the position, or generic attribute n
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Psiz,
    Pos,
    Gen(i64),
}

impl Key {
    fn to_json(self) -> Value {
        match self {
            Key::Psiz => json!("psiz"),
            Key::Pos => Value::Null,
            Key::Gen(n) => json!(n),
        }
    }
    fn from_json(v: &Value) -> Option<Key> {
        match v {
            Value::Null => Some(Key::Pos),
            Value::String(s) if s == "psiz" => Some(Key::Psiz),
            v => v.as_i64().map(Key::Gen),
        }
    }
}

/// the facts of a task or mesh shader (`info`'s dict, with the keys `to_stage` adds: `spill`; `refused` is the build driver's)
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MeshInfo {
    /// `ts` or `ms` (the Python's `None` for any other control byte is `Err` in `info`)
    pub kind: String,
    pub threads: i64,
    pub payload: i64,
    // mesh shaders only (the Python dict has these keys only for `ms`)
    pub task: Option<bool>,
    pub verts: Option<i64>,
    pub max_v: i64,
    pub max_p: i64,
    pub vertex: Vec<(Key, i64)>,
    pub primitive: Vec<(i64, i64)>,
    pub primitive_system: Option<i64>,
    pub primitive_builtins: Option<Vec<i64>>,
    pub spill: bool,
    pub refused: Option<String>,
}

impl MeshInfo {
    /// the dict as `json.dump` writes it, in the Python dict's insertion order
    pub fn to_json(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("kind".into(), json!(self.kind));
        m.insert("threads".into(), json!(self.threads));
        m.insert("payload".into(), json!(self.payload));
        if self.kind == "ms" {
            if let Some(t) = self.task {
                m.insert("task".into(), json!(t));
            }
            if let Some(v) = self.verts {
                m.insert("verts".into(), json!(v));
            }
            m.insert("max_v".into(), json!(self.max_v));
            m.insert("max_p".into(), json!(self.max_p));
            m.insert(
                "vertex".into(),
                Value::Array(
                    self.vertex
                        .iter()
                        .map(|(k, c)| json!([k.to_json(), c]))
                        .collect(),
                ),
            );
            m.insert(
                "primitive".into(),
                Value::Array(self.primitive.iter().map(|(n, c)| json!([n, c])).collect()),
            );
            if let Some(p) = self.primitive_system {
                m.insert("primitive_system".into(), json!(p));
            }
            if let Some(b) = &self.primitive_builtins {
                m.insert("primitive_builtins".into(), json!(b));
            }
        }
        if self.spill {
            m.insert("spill".into(), json!(1));
        }
        if let Some(r) = &self.refused {
            m.insert("refused".into(), json!(r));
        }
        Value::Object(m)
    }

    /// from the job's `mesh_stage` object
    pub fn from_json(v: &Value) -> Result<MeshInfo, String> {
        let o = v.as_object().ok_or("mesh_stage is not an object")?;
        let int = |k: &str| o.get(k).and_then(|x| x.as_i64());
        let kind = o
            .get("kind")
            .and_then(|x| x.as_str())
            .ok_or("KeyError: 'kind'")?
            .to_string();
        let mut i = MeshInfo {
            kind,
            threads: int("threads").ok_or("KeyError: 'threads'")?,
            payload: int("payload").ok_or("KeyError: 'payload'")?,
            ..Default::default()
        };
        if i.kind == "ms" {
            i.task = o.get("task").and_then(|x| x.as_bool());
            i.verts = int("verts");
            i.max_v = int("max_v").ok_or("KeyError: 'max_v'")?;
            i.max_p = int("max_p").ok_or("KeyError: 'max_p'")?;
            for (k, dst) in [("vertex", 0), ("primitive", 1)] {
                for e in o
                    .get(k)
                    .and_then(|x| x.as_array())
                    .ok_or_else(|| format!("KeyError: '{k}'"))?
                {
                    let a = e
                        .as_array()
                        .filter(|a| a.len() == 2)
                        .ok_or("a component is not a pair")?;
                    let c = a[1]
                        .as_i64()
                        .ok_or("a component number is not an integer")?;
                    let key = Key::from_json(&a[0]).ok_or("a component key is not an integer")?;
                    if dst == 0 {
                        i.vertex.push((key, c));
                    } else if let Key::Gen(n) = key {
                        i.primitive.push((n, c));
                    } else {
                        return Err("a primitive component key is not an integer".into());
                    }
                }
            }
            i.primitive_system = int("primitive_system");
            i.primitive_builtins = o
                .get("primitive_builtins")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_i64()).collect());
        }
        i.spill = o
            .get("spill")
            .map(|x| !x.is_null() && x != &json!(0) && x != &json!(false))
            .unwrap_or(false);
        i.refused = o
            .get("refused")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        Ok(i)
    }
}

/// `is_stage(ctl)`: `ts`, `ms` or None, by the control section's stage byte
pub fn is_stage(ctl: &[u8]) -> Option<&'static str> {
    if ctl.len() > 0x56 {
        match ctl[0x56] {
            6 => Some("ts"),
            7 => Some("ms"),
            _ => None,
        }
    } else {
        None
    }
}

fn byte(sph: &[u8], i: usize) -> Result<u8, String> {
    sph.get(i)
        .copied()
        .ok_or_else(|| "IndexError: index out of range".to_string())
}

/// `sph_outputs(sph)`: the components an SPH's OMAP enables, in the order of the attribute area
pub fn sph_outputs(sph: &[u8]) -> Result<Vec<(Key, i64)>, String> {
    let mut out = Vec::new();
    let b35 = byte(sph, 0x35)?;
    if b35 >> 3 & 1 != 0 {
        out.push((Key::Psiz, 0));
    }
    for c in 0..4 {
        if b35 >> (4 + c) & 1 != 0 {
            out.push((Key::Pos, c));
        }
    }
    for n in 0..32usize {
        let b = byte(sph, 0x36 + n / 2)?;
        for c in 0..4 {
            if b >> (4 * (n % 2) + c) & 1 != 0 {
                out.push((Key::Gen(n as i64), c as i64));
            }
        }
    }
    Ok(out)
}

/// `sph_system(sph)`: the system-value components an SPH's OMAP enables besides the position, lowest bit first
pub fn sph_system(sph: &[u8]) -> Result<Vec<i64>, String> {
    let bits = byte(sph, 0x34)? as u32 | ((byte(sph, 0x35)? & 0xf) as u32) << 8;
    Ok((0..12).filter(|b| bits >> b & 1 != 0).collect())
}

/// `bytes[a:b]` with Python's clamping
fn slice(b: &[u8], a: usize, e: usize) -> &[u8] {
    let e = e.min(b.len());
    if a >= e { &[] } else { &b[a..e] }
}

/// `info(ctl, area)`: a task or mesh shader's facts from its control section and its whole code area
pub fn info(ctl: &[u8], area: &[u8]) -> Result<MeshInfo, String> {
    let kind = is_stage(ctl);
    let u16at = |o: usize| {
        ctl.get(o..o + 2)
            .map(|s| u16::from_le_bytes([s[0], s[1]]) as i64)
            .ok_or_else(|| {
                "struct.error: unpack_from requires a buffer of at least 92 bytes".to_string()
            })
    };
    let (threads, payload) = (u16at(0x58)?, u16at(0x5a)?);
    let Some(kind) = kind else {
        // the Python's dict has kind None; every later use of it raises
        return Ok(MeshInfo {
            kind: String::new(),
            threads,
            payload,
            ..Default::default()
        });
    };
    let mut out = MeshInfo {
        kind: kind.to_string(),
        threads,
        payload,
        ..Default::default()
    };
    if kind == "ms" {
        let (max_v, max_p) = (u16at(0x60)?, u16at(0x62)?);
        let flag = *ctl.get(0x68).ok_or("IndexError: index out of range")?;
        let at = if flag & 0x80 != 0 {
            ctl.get(0x64..0x68)
                .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
                .ok_or("struct.error")?
        } else {
            0
        };
        let prim_sph = slice(area, at, at + 0x80);
        let prim = if at != 0 {
            sph_outputs(prim_sph)?
        } else {
            vec![]
        };
        out.task = Some(*ctl.get(0x5e).ok_or("IndexError: index out of range")? != 0);
        let v = *ctl.get(0x5f).ok_or("IndexError: index out of range")?;
        out.verts = Some(if v != 0 { v as i64 } else { 3 });
        out.max_v = max_v;
        out.max_p = max_p;
        out.vertex = sph_outputs(slice(area, 0, 0x80))?;
        out.primitive = prim
            .iter()
            .filter_map(|(k, c)| {
                if let Key::Gen(n) = k {
                    Some((*n, *c))
                } else {
                    None
                }
            })
            .collect();
        let sys = if at != 0 {
            sph_system(prim_sph)?
        } else {
            vec![]
        };
        out.primitive_system = Some(sys.len() as i64);
        out.primitive_builtins = Some(sys);
    }
    Ok(out)
}

fn blocks_of(n: i64) -> i64 {
    (n.max(1) + BLOCK - 1) / BLOCK
}

fn per_primitive(i: &MeshInfo) -> i64 {
    i.primitive_system.unwrap_or(0) + i.primitive.len() as i64
}

/// `memory(i)`: (the stage memory's offsets for the translator: the workgroup's number, the input, the outputs, the primitive bytes [, ...]; the shared memory's bytes)
pub fn memory(i: &MeshInfo) -> (Vec<i64>, i64) {
    if i.kind == "ts" {
        let out = INPUT; // a task shader reads no ISBERD 0x30..0x38 input: its outputs follow word 0's block
        return (
            vec![WORKGROUP, INPUT, out, out + i.payload],
            out + ((i.payload + 3) & !3),
        );
    }
    let out = if i.task.unwrap_or(false) && payload_spill(i) {
        INPUT
    } else {
        INPUT + ((i.payload + 15) & !15)
    };
    let (v_blocks, p_blocks) = (blocks_of(i.max_v), blocks_of(i.max_p));
    let prim =
        out + (v_blocks * i.vertex.len() as i64 + p_blocks * per_primitive(i)).max(1) * BLOCK * 4;
    let n = (i.vertex.len() as i64).max(1);
    (
        vec![WORKGROUP, INPUT, out, prim, prim - out, n, i.max_v * 4 * n],
        prim + ((4 + i.verts.unwrap_or(3) * p_blocks * BLOCK + 3) & !3),
    )
}

/// `own_shared(code)`: whether the instructions use shared memory of their own (LDS, STS, ATOMS: Ampere opcodes as DecoderAmpere.cs marks them)
pub fn own_shared(code: &[u8]) -> bool {
    let mut o = 0usize;
    while o + 15 < code.len() {
        let op = u16::from_le_bytes([code[o], code[o + 1]]) & 0xfff;
        if matches!(op, 0x984 | 0x388 | 0x988 | 0x38c | 0x98c | 0xf8c) {
            return true;
        }
        o += 16;
    }
    false
}

/// `pixel_reads(sph)`: {generic attribute n: the mask of its components a pixel shader reads} from its SPH's input map (byte 0x18 + n, two bits a component)
pub fn pixel_reads(sph: &[u8]) -> Result<BTreeMap<i64, i64>, String> {
    let mut out = BTreeMap::new();
    for n in 0..32usize {
        let b = byte(sph, 0x18 + n)?;
        if b != 0 {
            out.insert(
                n as i64,
                (0..4)
                    .filter(|c| b >> (2 * c) & 3 != 0)
                    .map(|c| 1i64 << c)
                    .sum(),
            );
        }
    }
    Ok(out)
}

/// `{n: [index in comps or None, for x, y, z, w]}` in the order of first appearance (`_members`)
fn members_v(comps: &[(Key, i64)]) -> Vec<(Key, [Option<i64>; 4])> {
    let mut keys: Vec<Key> = Vec::new();
    for (k, _) in comps {
        if !keys.contains(k) {
            keys.push(*k);
        }
    }
    keys.into_iter()
        .map(|n| {
            (
                n,
                std::array::from_fn(|c| {
                    comps
                        .iter()
                        .position(|x| *x == (n, c as i64))
                        .map(|p| p as i64)
                }),
            )
        })
        .collect()
}

/// `_width(written, read)`: the components an output needs
fn width(written: &[Option<i64>; 4], read: Option<i64>) -> i64 {
    let hi = (0..4)
        .filter(|c| written[*c].is_some())
        .map(|c| c as i64 + 1)
        .max()
        .unwrap_or(0);
    let need4 = if (read.unwrap_or(0xf) & 8) != 0 && written[3].is_none() {
        4
    } else {
        0
    };
    hi.max(need4).max(1)
}

/// `_value(width, base)`
fn value(width: i64, base: &[Option<String>; 4]) -> String {
    let parts: Vec<String> = (0..width as usize)
        .map(|k| match base.get(k).and_then(|b| b.as_ref()) {
            Some(b) => format!("as_type<float>({b})"),
            None => (if k == 3 { "1.0" } else { "0.0" }).to_string(),
        })
        .collect();
    if width == 1 {
        parts[0].clone()
    } else {
        format!("float{width}({})", parts.join(", "))
    }
}

fn ty(width: i64) -> String {
    if width == 1 {
        "float".into()
    } else {
        format!("float{width}")
    }
}

/// `_params(text)`: a parameter list split at its top-level commas
pub(crate) fn params_of(text: &str) -> Vec<String> {
    let (mut out, mut depth, mut cur) = (Vec::new(), 0i32, String::new());
    for ch in text.chars() {
        if ch == ',' && depth == 0 {
            out.push(strip(&cur).to_string());
            cur.clear();
            continue;
        }
        if matches!(ch, '(' | '[' | '<') {
            depth += 1;
        }
        if matches!(ch, ')' | ']' | '>') {
            depth -= 1;
        }
        cur.push(ch);
    }
    if !strip(&cur).is_empty() {
        out.push(strip(&cur).to_string());
    }
    out
}

re!(RE_NAME, r"(\w+)\s*(?:\[\[.*\]\])?$");
re!(
    RE_THREAD,
    r"^(uint3?)\s+(\w+)\s*\[\[(thread_position_in_threadgroup|threadgroup_position_in_grid)\]\]$"
);
re!(RE_ATTRS, r"\s*\[\[.*\]\]$");
re!(RE_KERNEL_HEAD, r"kernel\s+void\s+main0\s*\(");
re!(RE_KERNEL_OPEN, r"^\s*\{[ \t]*\n");
re!(
    RE_KERNEL_TAIL,
    r"^[ \t]*threadgroup\s+spvUnsafeArray<uint,\s*(\d+)>\s+(\w+);\n"
);

/// the `_KERNEL` match: (start, end, parameters, declarations before the array, the array's words, its name)
fn find_kernel(msl: &str) -> Option<(usize, usize, String, String, String, String)> {
    for h in RE_KERNEL_HEAD.find_iter(msl) {
        // `((?:[^()]|\([^()]*\))*)\)`: the parameters end at the first `)` outside a one-level group
        let b = msl.as_bytes();
        let mut i = h.end();
        let params_start = i;
        let ok = loop {
            match b.get(i) {
                None => break false,
                Some(b')') => break true,
                Some(b'(') => {
                    let mut j = i + 1;
                    while j < b.len() && b[j] != b'(' && b[j] != b')' {
                        j += 1;
                    }
                    if j < b.len() && b[j] == b')' {
                        i = j + 1;
                    } else {
                        break false;
                    }
                }
                Some(_) => i += 1,
            }
        };
        if !ok {
            continue;
        }
        let params = &msl[params_start..i];
        let Some(open) = RE_KERNEL_OPEN.find(&msl[i + 1..]) else {
            continue;
        };
        let mut pos = i + 1 + open.end();
        let decl_start = pos;
        loop {
            if let Some(t) = RE_KERNEL_TAIL.captures(&msl[pos..]) {
                let decl = &msl[decl_start..pos];
                return Some((
                    h.start(),
                    pos + t.get(0).unwrap().end(),
                    params.to_string(),
                    decl.to_string(),
                    t[1].to_string(),
                    t[2].to_string(),
                ));
            }
            // one more declaration line: `[ \t]+[^\n{}]*;[ \t]*\n`, not starting with `threadgroup`
            let rest = &msl[pos..];
            let Some(nl) = rest.find('\n') else { break };
            let line = &rest[..nl];
            let trimmed = line.trim_start_matches([' ', '\t']);
            let leading = line.len() - trimmed.len();
            let is_tg = trimmed
                .strip_prefix("threadgroup")
                .map(|r| {
                    !r.chars()
                        .next()
                        .map(|c| c.is_alphanumeric() || c == '_')
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            if leading == 0
                || is_tg
                || line.contains('{')
                || line.contains('}')
                || !line.trim_end_matches([' ', '\t']).ends_with(';')
            {
                break;
            }
            pos += nl + 1;
        }
    }
    None
}

/// `re.search(rf'(?<![.\w]){re.escape(name)}(?!\s*\[)(?=\s*[,)])', text)`: the name handed on whole (to a helper function), not indexed
fn passed_on(text: &str, name: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).any(|(at, _)| {
        if text[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c == '.' || word(c))
        {
            return false;
        }
        let after = text[at + name.len()..].trim_start_matches(super::pycompat::is_py_space);
        after.starts_with(',') || after.starts_with(')')
    })
}

/// `_device(msl)`: the text with its threadgroup address space turned to device, as `tg_spill` does a kernel's
fn device(msl: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for ln in msl.split('\n') {
        if ln.contains("operator [] (size_t pos)") {
            out.push(ln.to_string());
            continue;
        }
        let l1 = replace_threadgroup_word(ln);
        out.push(replace_mem_flags(&l1));
    }
    out.join("\n")
}

/// `re.sub(r'\bthreadgroup\b(?=\s)', 'device', ln)`
fn replace_threadgroup_word(ln: &str) -> String {
    let b = ln.as_bytes();
    let mut out = String::with_capacity(ln.len());
    let mut i = 0usize;
    while i < b.len() {
        if ln[i..].starts_with("threadgroup") && (i == 0 || !crate::text::is_word(b[i - 1])) {
            let e = i + "threadgroup".len();
            if let Some(c) = ln[e..].chars().next()
                && super::pycompat::is_py_space(c)
            {
                out.push_str("device");
                i = e;
                continue;
            }
        }
        let c = ln[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `re.sub(r'(?<!mem_device \| )mem_flags::mem_threadgroup\b', 'mem_flags::mem_device | mem_flags::mem_threadgroup', ln)`
fn replace_mem_flags(ln: &str) -> String {
    const PAT: &str = "mem_flags::mem_threadgroup";
    let b = ln.as_bytes();
    let mut out = String::with_capacity(ln.len());
    let mut i = 0usize;
    while i < b.len() {
        if ln[i..].starts_with(PAT) {
            let e = i + PAT.len();
            let boundary = e >= b.len() || !crate::text::is_word(b[e]);
            if boundary && !ln[..i].ends_with("mem_device | ") {
                out.push_str("mem_flags::mem_device | mem_flags::mem_threadgroup");
                i = e;
                continue;
            }
        }
        let c = ln[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `primitive_builtins(i)`: [(the member's name, its Metal attribute, its component in a primitive block)] of the per-primitive system values the mesh function hands on
fn primitive_builtins(i: &MeshInfo) -> Vec<(&'static str, &'static str, i64)> {
    i.primitive_builtins
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == 8)
        .map(|(k, _)| ("dm_pid", "primitive_id", k as i64))
        .collect()
}

/// `to_stage(msl, i, reads)`: the translated kernel as the object function main0 (kind `ts`), or the mesh functions main0 and main0_z (kind `ms`, plus `main0_o` for a mesh shader
/// with no task). `reads`: {user(locnN) location: component mask} of what the paired pixel shaders read, None when not known. Sets `i.spill`.
pub fn to_stage(
    msl: &str,
    i: &mut MeshInfo,
    reads: Option<&BTreeMap<i64, i64>>,
) -> Result<String, String> {
    let kind = i.kind.clone();
    let Some((ks, ke, params_text, local, words_s, mem)) = find_kernel(msl) else {
        return Err(format!(
            "{kind}: no kernel main0 starting with its stage memory"
        ));
    };
    let params = params_of(&params_text);
    let words: i64 = words_s
        .parse()
        .map_err(|_| format!("{kind}: stage memory of {words_s} words is out of range"))?;
    let mut names: Vec<String> = Vec::new();
    for p in &params {
        match RE_NAME.captures(p) {
            Some(m) => names.push(m[1].to_string()),
            None => return Err(format!("no name in the kernel parameter {p}")),
        }
    }
    let plain: Vec<String> = params
        .iter()
        .map(|p| RE_ATTRS.replace(p, "").into_owned())
        .collect();
    let local = strip(&local).to_string();
    let pspill = (kind == "ts" || i.task == Some(true)) && payload_spill(i);
    let slice_w = payload_words(i);
    let mut rest = &msl[ke..];
    // A payload-spilled mesh shader's input: its dm_stage_in array leaves the body, which reads the task's device slice through dm_in_t instead (word 0 its workgroup's number)
    let mut sin: Option<String> = None;
    if pspill
        && kind == "ms"
        && let Some(n) = RE_KERNEL_TAIL.captures(rest)
    {
        if n[1].parse::<i64>().ok() != Some(slice_w) {
            return Err(format!(
                "ms: dm_stage_in has {} words, expected {slice_w}",
                &n[1]
            ));
        }
        let name = n[2].to_string();
        rest = &rest[n.get(0).unwrap().end()..];
        if passed_on(rest, &name) {
            return Err("ms: dm_stage_in is passed to a helper function".into());
        }
        sin = Some(name);
    }
    let mut all_plain = plain.clone();
    all_plain.push(format!("threadgroup spvUnsafeArray<uint, {words}>& {mem}"));
    if let Some(s) = &sin {
        all_plain.push(format!("dm_in_t {s}"));
    }
    let in_t = if sin.is_some() {
        "struct dm_in_t { const device uint* p; uint w0; uint operator[](int k) const { return k ? p[k] : w0; } uint operator[](uint k) const { return k ? p[k] : w0; } };\n"
    } else {
        ""
    };
    let mut src = format!(
        "{}{in_t}static inline __attribute__((always_inline)) void dm_stage_body({})\n{{\n{}{}",
        &msl[..ks],
        all_plain.join(", "),
        if local.is_empty() {
            String::new()
        } else {
            format!("    {local}\n")
        },
        rest
    );
    let spill = kind == "ms"
        && i.threads == 32
        && SPILL_MIN <= words * 4
        && words * 4 <= abi::MESH_SCRATCH_STRIDE;
    if spill {
        i.spill = true;
        src = device(&src);
    }
    let mut call_args = names.clone();
    call_args.push(mem.clone());
    if sin.is_some() {
        call_args.push("dm_in".into());
    }
    let call = format!("dm_stage_body({});", call_args.join(", "));
    let mut own: BTreeMap<String, String> = BTreeMap::new();
    for p in &params {
        if let Some(t) = RE_THREAD.captures(p) {
            own.insert(
                t[3].to_string(),
                format!("{}{}", &t[2], if &t[1] == "uint3" { ".x" } else { "" }),
            );
        }
    }
    let lid = own
        .get("thread_position_in_threadgroup")
        .cloned()
        .unwrap_or_else(|| "dm_lid".into());
    let tg = own
        .get("threadgroup_position_in_grid")
        .cloned()
        .unwrap_or_else(|| "dm_tg".into());
    let mut extra: Vec<String> = Vec::new();
    for (a, v) in [
        ("thread_position_in_threadgroup", "dm_lid"),
        ("threadgroup_position_in_grid", "dm_tg"),
    ] {
        if !own.contains_key(a) {
            extra.push(format!("uint {v} [[{a}]]"));
        }
    }
    let (lay, _) = memory(i);
    let (thr, pay) = (i.threads, (i.payload + 3) >> 2);
    let (wg, inp, out, prim) = (lay[0] >> 2, lay[1] >> 2, lay[2] >> 2, lay[3]);
    let bar = if spill {
        "mem_flags::mem_device | mem_flags::mem_threadgroup"
    } else {
        "mem_flags::mem_threadgroup"
    };
    let fence = "    atomic_thread_fence(mem_flags::mem_device, memory_order_seq_cst, thread_scope_device);\n";
    let (start, release): (String, String);
    if spill {
        let w4 = words / 4;
        let h = "(device atomic_uint*)dm_scr";
        let flag = format!("{h} + {}u + dm_t", abi::MESH_SCRATCH_FLAGS / 4);
        let mut s = String::new();
        s.push_str("    uint dm_t = ~0u;\n");
        s.push_str(&format!("    if ({lid} == 0u)\n    {{\n"));
        s.push_str(&format!("        uint dm_m = atomic_load_explicit({h} + {}, memory_order_relaxed), dm_tag = atomic_load_explicit({h} + {}, memory_order_relaxed);\n", abi::MESH_SCRATCH_MASK, abi::MESH_SCRATCH_TAG));
        s.push_str(&format!(
            "        uint dm_done = atomic_load_explicit({h} + {}, memory_order_relaxed);\n",
            abi::MESH_SCRATCH_DONE
        ));
        s.push_str(&format!(
            "        for (uint dm_k = 0u; dm_k < {}u; dm_k++)\n        {{\n",
            abi::MESH_SCRATCH_TRIES
        ));
        s.push_str(&format!("            uint dm_s = atomic_fetch_add_explicit({h} + {}, 1u, memory_order_relaxed) & dm_m;\n", abi::MESH_SCRATCH_TICKET));
        s.push_str(&format!(
            "            device atomic_uint* dm_f = {h} + {}u + dm_s;\n",
            abi::MESH_SCRATCH_FLAGS / 4
        ));
        s.push_str("            uint dm_v = atomic_load_explicit(dm_f, memory_order_relaxed);\n");
        s.push_str("            if ((dm_v == 0u || int(dm_done - dm_v) >= 0) && atomic_compare_exchange_weak_explicit(dm_f, &dm_v, dm_tag, memory_order_relaxed, memory_order_relaxed)) { dm_t = dm_s; break; }\n");
        s.push_str("        }\n");
        s.push_str(&format!("        if (dm_t == ~0u) atomic_fetch_add_explicit({h} + {}, 1u, memory_order_relaxed);\n    }}\n", abi::MESH_SCRATCH_OVER));
        s.push_str("    dm_t = simd_broadcast_first(dm_t);\n");
        s.push_str(&format!("    if (dm_t == ~0u)\n    {{\n        if ({lid} == 0u) dm_out.set_primitive_count(0u);\n        return;\n    }}\n"));
        s.push_str(fence);
        s.push_str(&format!("    device spvUnsafeArray<uint, {words}>& {mem} = *(device spvUnsafeArray<uint, {words}>*)(dm_scr + {}ul + ulong(dm_t) * {}ul);\n", abi::MESH_SCRATCH_HEAD, abi::MESH_SCRATCH_STRIDE));
        s.push_str(&format!("    for (uint i = {lid}; i < {w4}u; i += {thr}u) ((device uint4*)&{mem}[0])[i] = uint4(0u);\n"));
        if words % 4 != 0 {
            s.push_str(&format!(
                "    for (uint i = {}u + {lid}; i < {words}u; i += {thr}u) {mem}[i] = 0u;\n",
                w4 * 4
            ));
        }
        s.push_str(&format!("    threadgroup_barrier({bar});\n"));
        start = s;
        release = format!(
            "    threadgroup_barrier({bar});\n{fence}    if ({lid} == 0u) atomic_store_explicit({flag}, 0u, memory_order_relaxed);\n"
        );
    } else {
        start = format!(
            "    threadgroup spvUnsafeArray<uint, {words}> {mem};\n    for (uint i = {lid}; i < {words}u; i += {thr}u) {mem}[i] = 0u;\n    threadgroup_barrier({bar});\n"
        );
        release = String::new();
    }
    let mut head = if pspill {
        "struct dm_payload_t { ulong off; };\n".to_string()
    } else {
        format!("struct dm_payload_t {{ uint w[{}]; }};\n", pay.max(1))
    };
    let mut sig_v = params.clone();
    sig_v.extend(extra.iter().cloned());
    if pspill && (kind == "ts" || sin.is_some()) {
        sig_v.push(format!(
            "{} uint* dm_spill [[buffer({MESH_SPILL_SLOT})]]",
            if kind == "ts" {
                "device"
            } else {
                "const device"
            }
        ));
    }
    let sig = sig_v.join(", ");
    let draw = format!("constant uint* dm_draw [[buffer({})]]", abi::MESH_DRAW_SLOT);
    let scr = if spill {
        format!(
            ", device uchar* dm_scr [[buffer({})]]",
            abi::MESH_SCRATCH_SLOT
        )
    } else {
        String::new()
    };
    if kind == "ts" {
        let store = if pspill {
            format!(
                "    if ({tg} >= dm_spill[0]) {{ if ({lid} == 0u) dm_grid.set_threadgroups_per_grid(uint3(0u, 1u, 1u)); return; }}\n    ulong dm_off = 4ul + ulong({tg}) * {slice_w}ul;\n    device uint* dm_slice = dm_spill + dm_off;\n    for (uint i = {lid}; i < {pay}u; i += {thr}u) dm_slice[i] = {mem}[{out}u + i];\n    threadgroup_barrier(mem_flags::mem_device);\n{fence}    if ({lid} == 0u) dm_payload.off = dm_off;\n"
            )
        } else {
            format!(
                "    for (uint i = {lid}; i < {pay}u; i += {thr}u) dm_payload.w[i] = {mem}[{out}u + i];\n"
            )
        };
        return Ok(format!(
            "{src}\n{head}[[object]] void main0({sig}, object_data dm_payload_t& dm_payload [[payload]], mesh_grid_properties dm_grid, {draw})\n{{\n{start}    if ({lid} == 0u) {mem}[{wg}] = dm_draw[1] + {tg};\n    threadgroup_barrier({bar});\n    {call}\n    threadgroup_barrier({bar});\n{store}    if ({lid} == 0u) dm_grid.set_threadgroups_per_grid(uint3(min({mem}[{}], {METAL_MAX_GRID}u), 1u, 1u));\n}}\n",
            out + 1
        ));
    }
    let (vertex, primitive) = (&i.vertex, &i.primitive);
    let (max_v, max_p) = (i.max_v, i.max_p);
    let verts = i.verts.unwrap_or(3);
    if !(1..=3).contains(&verts) {
        return Err(format!(
            "ms: primitive vertex count {verts} is not 1, 2 or 3"
        ));
    }
    if max_v > METAL_MAX_VERTICES || max_p > METAL_MAX_PRIMITIVES {
        return Err(format!(
            "ms: {max_v} vertices and {max_p} primitives exceed a Metal mesh"
        ));
    }
    let topology = match verts {
        1 => "point",
        2 => "line",
        _ => "triangle",
    };
    let v_blocks = (max_v.max(1) + BLOCK - 1).max(0) / BLOCK;
    let nsys = i.primitive_system.unwrap_or(0);
    let (v_stride, p_stride) = (
        (vertex.len() as i64).max(1) * BLOCK,
        per_primitive(i).max(1) * BLOCK,
    );
    let p_out = out + v_blocks * vertex.len() as i64 * BLOCK;
    let word = |comps: &[Option<i64>; 4]| -> [Option<String>; 4] {
        std::array::from_fn(|k| comps[k].map(|c| format!("{mem}[b + {}u]", c * BLOCK)))
    };
    let vm = members_v(vertex);
    let pm: Vec<(i64, [Option<i64>; 4])> = {
        let pc: Vec<(Key, i64)> = primitive.iter().map(|(n, c)| (Key::Gen(*n), *c)).collect();
        members_v(&pc)
            .into_iter()
            .map(|(k, v)| {
                (
                    if let Key::Gen(n) = k {
                        n
                    } else {
                        unreachable!()
                    },
                    std::array::from_fn(|c| v[c].map(|x| x + nsys)),
                )
            })
            .collect()
    };
    let pb = primitive_builtins(i);
    let read = |n: i64| -> Option<i64> { reads.map(|r| r.get(&n).copied().unwrap_or(0)) };
    let vm_ints: Vec<i64> = vm
        .iter()
        .filter_map(|(k, _)| if let Key::Gen(n) = k { Some(*n) } else { None })
        .collect();
    let mut miss: Vec<i64> = reads
        .map(|r| {
            r.keys()
                .copied()
                .filter(|n| !vm_ints.contains(n) && !pm.iter().any(|(k, _)| k == n))
                .collect()
        })
        .unwrap_or_default();
    miss.sort();
    let vw: Vec<(i64, i64)> = vm
        .iter()
        .filter_map(|(k, c)| {
            if let Key::Gen(n) = k {
                Some((*n, width(c, read(*n))))
            } else {
                None
            }
        })
        .collect();
    let pw: Vec<(i64, i64)> = pm.iter().map(|(n, c)| (*n, width(c, read(*n)))).collect();
    let mw: Vec<(i64, i64)> = miss
        .iter()
        .map(|n| (*n, width(&[None; 4], read(*n))))
        .collect();
    let vwid = |n: i64| vw.iter().find(|(k, _)| *k == n).map(|x| x.1).unwrap_or(1);
    let pwid = |n: i64| pw.iter().find(|(k, _)| *k == n).map(|x| x.1).unwrap_or(1);
    // the position is invariant, as a vertex shader's
    head.push_str("struct dm_vertex_t {\n    float4 position [[position, invariant]];\n");
    if vm.iter().any(|(k, _)| *k == Key::Psiz) {
        head.push_str("    float point_size [[point_size]];\n");
    }
    for (n, w) in &vw {
        head.push_str(&format!("    {} a{n} [[user(locn{n})]];\n", ty(*w)));
    }
    for (n, w) in &mw {
        head.push_str(&format!("    {} dm_pad{n} [[user(locn{n})]];\n", ty(*w)));
    }
    head.push_str("};\n");
    if !pm.is_empty() || !pb.is_empty() {
        head.push_str("struct dm_prim_t {\n");
        for (name, attr, _) in &pb {
            head.push_str(&format!("    uint {name} [[{attr}]];\n"));
        }
        for (n, w) in &pw {
            head.push_str(&format!("    {} p{n} [[user(locn{n})]];\n", ty(*w)));
        }
        head.push_str("};\n");
    }
    head.push_str(&format!("using dm_mesh_t = metal::mesh<dm_vertex_t, {}, {max_v}, {max_p}, metal::topology::{topology}>;\n", if !pm.is_empty() || !pb.is_empty() { "dm_prim_t" } else { "void" }));
    let v_block = |v: &str| format!("{out}u + ({v} / {BLOCK}u) * {v_stride}u + {v} % {BLOCK}u");
    let p_block = |p: &str| format!("{p_out}u + ({p} / {BLOCK}u) * {p_stride}u + {p} % {BLOCK}u");
    let mut sets = String::new();
    for (n, c) in &vm {
        let w = word(c);
        match n {
            Key::Psiz => sets.push_str(&format!(
                "        v.point_size = as_type<float>({});\n",
                w[0].clone().unwrap_or_else(|| "None".into())
            )),
            Key::Pos => sets.push_str(&format!("        v.position = {};\n", value(4, &w))),
            Key::Gen(g) => sets.push_str(&format!("        v.a{g} = {};\n", value(vwid(*g), &w))),
        }
    }
    for (n, w) in &mw {
        sets.push_str(&format!(
            "        v.dm_pad{n} = {};\n",
            value(*w, &[None, None, None, None])
        ));
    }
    let mut psets = String::new();
    for (name, _, k) in &pb {
        psets.push_str(&format!("        p.{name} = {mem}[b + {}u];\n", k * BLOCK));
    }
    for (n, c) in &pm {
        psets.push_str(&format!(
            "        p.p{n} = {};\n",
            value(pwid(*n), &word(c))
        ));
    }
    if !vm.iter().any(|(k, _)| *k == Key::Pos) {
        return Err("ms: its SPH enables no position component".into());
    }
    let task = i.task.ok_or("KeyError: 'task'")?;
    let entry = |name: &str, gl: bool| -> String {
        let fill = if task && pspill {
            format!(
                "{}{}    if ({lid} == 0u) {mem}[{wg}] = {tg};\n",
                if spill { "" } else { fence },
                if sin.is_some() {
                    format!("    dm_in_t dm_in = {{ dm_spill + dm_payload.off, {tg} }};\n")
                } else {
                    String::new()
                }
            )
        } else if task {
            format!(
                "    for (uint i = {lid}; i < {pay}u; i += {thr}u) {mem}[{inp}u + i] = dm_payload.w[i];\n    threadgroup_barrier({bar});\n    if ({lid} == 0u) {{ {mem}[{wg}] = {tg}; {mem}[{inp}] = {tg}; }}\n"
            )
        } else {
            format!("    if ({lid} == 0u) {mem}[{wg}] = dm_block.first + {tg};\n")
        };
        let payload_param = if task {
            ", const object_data dm_payload_t& dm_payload [[payload]]"
        } else {
            ", const object_data dm_block_t& dm_block [[payload]]"
        };
        let mut s = format!(
            "[[mesh]] void {name}({sig}, dm_mesh_t dm_out{payload_param}{scr})\n{{\n{start}{fill}"
        );
        s.push_str(&format!(
            "    threadgroup_barrier({bar});\n    {call}\n    threadgroup_barrier({bar});\n"
        ));
        s.push_str(&format!(
            "    uint np = min({mem}[{}] >> 24, {max_p}u);\n",
            prim >> 2
        ));
        s.push_str(&format!(
            "    if ({lid} == 0u) dm_out.set_primitive_count(np);\n"
        ));
        s.push_str(&format!("    for (uint n = {lid}; n < {max_v}u; n += {thr}u)\n    {{\n        uint b = {};\n        dm_vertex_t v;\n{sets}", v_block("n")));
        if gl {
            s.push_str("        v.position.z = fma(v.position.z, 0.5, v.position.w * 0.5);\n");
        }
        s.push_str("        dm_out.set_vertex(n, v);\n    }\n");
        if !pm.is_empty() || !pb.is_empty() {
            s.push_str(&format!("    for (uint n = {lid}; n < np; n += {thr}u)\n    {{\n        uint b = {};\n        dm_prim_t p;\n{psets}        dm_out.set_primitive(n, p);\n    }}\n", p_block("n")));
        }
        s.push_str(&format!("    for (uint n = {lid}; n < np * {verts}u; n += {thr}u)\n    {{\n        uint at = {}u + n;\n        dm_out.set_index(n, ({mem}[at >> 2] >> ((at & 3u) * 8u)) & 0xffu);\n    }}\n", prim + 4));
        s.push_str(&release);
        s.push_str("}\n");
        s
    };
    if task {
        return Ok(format!(
            "{src}\n{head}{}\n{}",
            entry("main0", true),
            entry("main0_z", false)
        ));
    }
    head.push_str("struct dm_block_t { uint first; };\n");
    let obj = format!(
        "[[object]] void main0_o(object_data dm_block_t& dm_block [[payload]], mesh_grid_properties dm_grid, uint dm_otg [[threadgroup_position_in_grid]], {draw})\n{{\n    uint b = dm_otg * {METAL_MAX_GRID}u;\n    dm_block.first = dm_draw[1] + b;\n    dm_grid.set_threadgroups_per_grid(uint3(min(dm_draw[0] - b, {METAL_MAX_GRID}u), 1u, 1u));\n}}\n"
    );
    Ok(format!(
        "{src}\n{head}{}\n{}\n{obj}",
        entry("main0", true),
        entry("main0_z", false)
    ))
}
