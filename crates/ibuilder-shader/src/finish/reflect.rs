//! `dmsh_reflect.py`: the reflection of a translated shader's MSL (`reflect`: the argument buffer -> the binding list; `drv_reads`, `unfillable`, `attributes`, `raster_facts`).
//! `pool_arrays` is a rewrite of the chain (F1), not here.
use super::abi;
use super::pyjson::J;
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

/// A recorded input variable with the two fma operands: ((struct, member), x, y).
type FmaOperand = ((String, String), [i64; 3], [i64; 3]);

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

#[inline]
pub fn is_word(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}
/// whether a word character comes just before `src[i]`
pub fn in_word(src: &str, i: usize) -> bool {
    i > 0 && is_word(src.as_bytes()[i - 1])
}
/// `dmsh_reflect.has_word`: `text` (which starts with a word character) is in `src` with no word character before it
pub fn has_word(src: &str, text: &str) -> bool {
    let mut from = 0;
    while let Some(p) = src[from..].find(text) {
        let i = from + p;
        if !in_word(src, i) {
            return true;
        }
        from = i + 1;
        while from < src.len() && !src.is_char_boundary(from) {
            from += 1;
        }
    }
    false
}

// ------------------------------------------------------------------------------------------ attributes
re!(RE_ATTR, r"(\w+)\s+(\w+)\s*\[\[attribute\((\d+)\)\]\]");

/// `attributes(src)`: (type, name, location) of the vertex attribute declarations, searched only in the lines from the first to the last
pub fn attributes(src: &str) -> Vec<(String, String, i64)> {
    let (Some(i), Some(j)) = (src.find("[[attribute("), src.rfind("[[attribute(")) else {
        return vec![];
    };
    let end = src[j..].find('\n').map(|p| p + j).unwrap_or(src.len());
    let start = src[..i].rfind('\n').map(|p| p + 1).unwrap_or(0);
    RE_ATTR
        .captures_iter(&src[start..end])
        .map(|m| {
            (
                m[1].to_string(),
                m[2].to_string(),
                m[3].parse().unwrap_or(0),
            )
        })
        .collect()
}

// ------------------------------------------------------------------------------------------ raster facts
re!(RE_CTRL, r"\b(?:if|for|while|switch|do)\b");
re!(
    RE_POS_ASSIGN_ANY,
    r"out\.gl_Position(?:\.[xyzw]{2,})?\s*=[^=]"
);
re!(RE_POS_ZW, r"out\.gl_Position\.([zw])\s*=\s*([^;]+);");
re!(RE_REG_NAME, r"^_\d+$");
re!(RE_FLOAT_LIT, r"^-?\d+\.\d*(?:e[+-]?\d+)?$");

/// number of `(?<![\w.])<name>\s*=[^=]` matches in `body` (non-overlapping, as `re.findall`)
fn count_single_assign(body: &str, name: &str) -> usize {
    let b = body.as_bytes();
    let (mut n, mut pos) = (0usize, 0usize);
    while let Some(p) = body[pos..].find(name) {
        let i = pos + p;
        let prev_ok = i == 0 || !(is_word(b[i - 1]) || b[i - 1] == b'.');
        if prev_ok {
            let mut k = i + name.len();
            while k < b.len() && matches!(b[k], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
                k += 1;
            }
            if k < b.len() && b[k] == b'=' && k + 1 < b.len() {
                // `[^=]` takes one whole character
                let rest = &body[k + 1..];
                if let Some(c) = rest.chars().next()
                    && c != '='
                {
                    n += 1;
                    pos = k + 1 + c.len_utf8();
                    continue;
                }
            }
        }
        pos = i + 1;
        while pos < body.len() && !body.is_char_boundary(pos) {
            pos += 1;
        }
    }
    n
}

pub fn far_plane(body: &str) -> bool {
    if RE_CTRL.is_match(body) || RE_POS_ASSIGN_ANY.is_match(body) {
        return false;
    }
    let mut last: HashMap<String, String> = HashMap::new();
    for m in RE_POS_ZW.captures_iter(body) {
        let v = m[2].trim().to_string();
        if &m[1] == "z" && v == "fma(out.gl_Position.z, 0.5, out.gl_Position.w * 0.5)" {
            continue;
        }
        last.insert(m[1].to_string(), v);
    }
    let Some(v) = last.get("z") else { return false };
    if v.is_empty() || Some(v) != last.get("w") || v == "0.0" || v == "-0.0" {
        return false;
    }
    if RE_REG_NAME.is_match(v) {
        return count_single_assign(body, v) == 1;
    }
    RE_FLOAT_LIT.is_match(v)
}

re!(RE_OUT_STRUCT, r"(?s)struct main0_out\s*\{(.*?)\};");
re!(RE_COLOR_IDX, r"\[\[color\((\d+)\)\]\]");
re!(
    RE_COLOR_DECL,
    r"(?m)^\s*(\w+)\s+\w+\s*\[\[color\((\d+)\)[^\]]*\]\];"
);
re!(
    RE_VS_BODY,
    r"(?s)\bvertex\s+\w+\s+\w+\s*\(.*?\)\s*\{(.*)\}\s*$"
);
re!(
    RE_VS_CTRL2,
    r"\b(?:if|for|while|switch|do)\b|clip_distance|ClipDistance"
);
re!(RE_IN_VAR, r"float\s+(_\w+)\s*=\s*in\.(\w+)\.([xyzw]);");
re!(RE_LAST_POS, r"out\.gl_Position\.([xyzw])\s*=\s*([^;]+);");
re!(RE_LOCN_OUT, r"(m_\d+) \[\[user\(locn(\d+)\)\]\]");
re!(RE_OUT_COMP, r"out\.(m_\d+)\.([xyzw])\s*=\s*([^;]+);");
re!(
    RE_FMA,
    r"float (\S+) = fma\((_\w+), \(\*spvDescriptorSet0\.vp_c(\d+)\)\._m0\[(\d+)\]\.([xyzw]), \(\*spvDescriptorSet0\.vp_c(\d+)\)\._m0\[(\d+)\]\.([xyzw])\);"
);

fn comp_index(c: &str) -> i64 {
    "xyzw".find(c).unwrap_or(0) as i64
}

/// `raster_facts(stage, src)`: the dict as an ordered `J`
pub fn raster_facts(stage: &str, src: &str) -> J {
    if stage == "ps" {
        let mem = RE_OUT_STRUCT
            .captures(src)
            .map(|c| c.get(1).unwrap().as_str().to_string())
            .unwrap_or_default();
        let colors: BTreeSet<i64> = RE_COLOR_IDX
            .captures_iter(&mem)
            .filter_map(|m| m[1].parse().ok())
            .collect();
        // the colour outputs main0 declares float, unsigned and signed (bitmaps)
        let bits = |prefixes: &[&str]| -> i64 {
            RE_COLOR_DECL
                .captures_iter(&mem)
                .filter(|m| prefixes.iter().any(|p| m[1].starts_with(p)))
                .map(|m| 1i64 << m[2].parse::<i64>().unwrap_or(0))
                .sum()
        };
        return J::obj()
            .with("discard", src.contains("discard_fragment") as i64)
            .with("sample_mask", mem.contains("sample_mask") as i64)
            .with("primitive_id", src.contains("[[primitive_id]]") as i64)
            .with("colors", J::Arr(colors.into_iter().map(J::Int).collect()))
            .with("float_colors", bits(&["float", "half"]))
            .with("uint_colors", bits(&["uint"]))
            .with("sint_colors", bits(&["int"]));
    }
    if stage != "vs" {
        return J::obj();
    }
    let mut scr = J::obj().with(
        "screen",
        J::Arr(
            super::fetch::screen_varyings(src)
                .into_iter()
                .map(J::Int)
                .collect(),
        ),
    );
    let body = RE_VS_BODY.captures(src).map(|c| c.get(1).unwrap().as_str());
    if let Some(b) = body
        && far_plane(b)
    {
        scr.set("far", 1);
    }
    let fail = |scr: &J| J::obj().with("pos_attr", -1).with_all(scr);
    let Some(b) = body else { return fail(&scr) };
    if RE_VS_CTRL2.is_match(b) {
        return fail(&scr);
    }
    let mut attr: HashMap<String, i64> = HashMap::new();
    for (_, n, l) in attributes(src) {
        attr.insert(n, l);
    }
    let mut var: HashMap<String, (String, String)> = HashMap::new();
    for m in RE_IN_VAR.captures_iter(b) {
        var.insert(m[1].to_string(), (m[2].to_string(), m[3].to_string()));
    }
    let mut last: HashMap<String, String> = HashMap::new();
    for m in RE_LAST_POS.captures_iter(b) {
        last.insert(m[1].to_string(), m[2].trim().to_string());
    }
    let src_of =
        |c: &str| -> Option<(String, String)> { last.get(c).and_then(|v| var.get(v)).cloned() };
    let Some(m0) = src_of("x") else {
        return fail(&scr);
    };
    if !attr.contains_key(&m0.0)
        || "xyz"
            .chars()
            .any(|c| src_of(&c.to_string()) != Some((m0.0.clone(), c.to_string())))
    {
        return fail(&scr);
    }
    let w = if last.get("w").map(|s| s.as_str()) == Some("1.0") {
        "one"
    } else if src_of("w") == Some((m0.0.clone(), "w".to_string())) {
        "attr"
    } else {
        return fail(&scr);
    };
    // uv pass-through and affine uv
    let ost = RE_OUT_STRUCT
        .captures(src)
        .map(|c| c.get(1).unwrap().as_str())
        .unwrap_or("");
    let mut locn: Vec<(String, i64)> = Vec::new();
    for m in RE_LOCN_OUT.captures_iter(ost) {
        let (n, k) = (m[1].to_string(), m[2].parse().unwrap_or(0));
        if let Some(e) = locn.iter_mut().find(|(x, _)| *x == n) {
            e.1 = k;
        } else {
            locn.push((n, k));
        }
    }
    let mut olast: HashMap<(String, String), String> = HashMap::new();
    for m in RE_OUT_COMP.captures_iter(b) {
        olast.insert(
            (m[1].to_string(), m[2].to_string()),
            m[3].trim().to_string(),
        );
    }
    let mut fma: HashMap<String, (String, [i64; 3], [i64; 3])> = HashMap::new();
    for m in RE_FMA.captures_iter(b) {
        fma.entry(m[1].to_string()).or_insert_with(|| {
            (
                m[2].to_string(),
                [
                    m[3].parse::<i64>().unwrap_or(0) - 3,
                    m[4].parse().unwrap_or(0),
                    comp_index(&m[5]),
                ],
                [
                    m[6].parse::<i64>().unwrap_or(0) - 3,
                    m[7].parse().unwrap_or(0),
                    comp_index(&m[8]),
                ],
            )
        });
    }
    // fm(v_): re.search(...) -> None unless the fma's first operand is a recorded input variable
    let fm = |v_: &str| -> Option<FmaOperand> {
        let (a, x, y) = fma.get(v_)?;
        let vv = var.get(a)?;
        Some((vv.clone(), *x, *y))
    };
    let (mut uv, mut aff) = (J::obj(), J::obj());
    for (mem, k) in &locn {
        let oxk = olast
            .get(&(mem.clone(), "x".to_string()))
            .map(|s| s.as_str())
            .unwrap_or("");
        let oyk = olast
            .get(&(mem.clone(), "y".to_string()))
            .map(|s| s.as_str())
            .unwrap_or("");
        let (sx, sy) = (var.get(oxk), var.get(oyk));
        if let (Some(sx), Some(sy)) = (sx, sy)
            && sx.0 == sy.0
            && attr.contains_key(&sx.0)
            && sx.1 == "x"
            && sy.1 == "y"
        {
            uv.set(&k.to_string(), attr[&sx.0]);
            continue;
        }
        if let (Some(fx), Some(fy)) = (fm(oxk), fm(oyk)) {
            let mn = [fx.1[0], fx.2[0], fy.1[0], fy.2[0]]
                .into_iter()
                .min()
                .unwrap_or(-1);
            if fx.0.0 == fy.0.0
                && attr.contains_key(&fx.0.0)
                && fx.0.1 == "x"
                && fy.0.1 == "y"
                && mn >= 0
            {
                let arr = |a: [i64; 3]| J::Arr(a.iter().map(|x| J::Int(*x)).collect());
                aff.set(
                    &k.to_string(),
                    J::obj()
                        .with("attr", attr[&fx.0.0])
                        .with("sx", arr(fx.1))
                        .with("ox", arr(fx.2))
                        .with("sy", arr(fy.1))
                        .with("oy", arr(fy.2)),
                );
            }
        }
    }
    J::obj()
        .with("pos_attr", attr[&m0.0])
        .with("pos_w", w)
        .with("uv_from_attr", uv)
        .with("uv_affine", aff)
        .with_all(&scr)
}

impl J {
    /// Python `{**self, **other}` as a builder
    pub fn with_all(mut self, other: &J) -> J {
        self.extend(other);
        self
    }
}

// ------------------------------------------------------------------------------------------ handle name decoders
re!(RE_DEC_TEX, r"^[a-z]+_[ti]\w*?_(?:cb(\d+)|tcb)_([0-9A-F]+)");
re!(RE_IS_IMG, r"^[a-z]+_i");
re!(RE_CONST_SAMP, r"^[a-z]+_t\w*?_(?:cb(\d+)|tcb)_([0-9A-F]+)");
re!(RE_UBO_TEX, r"^[a-z]+_t\w*?_cb(\d+)_([0-9A-F]+)");
re!(RE_UBO_SAMP_ID, r"^[a-z]+_t\w*?_cb(\d+)_([0-9A-F]+)$");
re!(RE_UBO_IMG, r"^[a-z]+_i\w*?_cb(\d+)_([0-9A-F]+)");
re!(
    RE_IMG_BIAS,
    r"^[a-z]+_([ti])[a-z0-9]*_(?:cb(\d+)|tcb)_([0-9A-F]+)(?:_|$)"
);
re!(RE_C2ARR, r"^[a-z]+_t\w*?_cb(\d+)_([0-9A-F]+)$");

/// `int(s, 16)` (the regexes give `[0-9A-F]+`), unbounded in Python. Exact below 2^124 (31 significant digits); past that the low 31 digits are kept and bit
/// 124 is set, which keeps every use the Python's: the uses read the low bits (`& 0x3fff`, `>> 14 & 0x3fff`) or compare `h >> 28` with a small handle type.
/// (An i64 parse made any handle >= 2^63 a 0, so type 0.)
fn hex(s: &str) -> i128 {
    let t = s.trim_start_matches('0');
    if t.len() <= 31 {
        return i128::from_str_radix(t, 16).unwrap_or(0); // "" (all zeros) is 0
    }
    i128::from_str_radix(&t[t.len() - 31..], 16).unwrap_or(0) | 1 << 124
}
/// an i128 value written where the json takes an i64: clamped (the Python's int has no bound; only a handle past 2^91 or an offset past 2^63 differs)
fn i64_of(v: i128) -> i64 {
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}
/// `(h & 0x3fff, (h >> 14) & 0x3fff, h >> 28)`: the handle's two word offsets and its type
fn handle_parts(h: i128) -> (i64, i64, i64) {
    (
        (h & 0x3fff) as i64,
        ((h >> 14) & 0x3fff) as i64,
        i64_of(h >> 28),
    )
}
fn dec(s: &str) -> i64 {
    s.parse().unwrap_or(i64::MAX / 4)
}

fn slot(word: i64, base: i64, cb: i64) -> Option<i64> {
    if cb == 2 && word * 4 >= base && (word * 4 - base) % abi::C2_ROW == 0 {
        Some((word * 4 - base) / abi::C2_ROW)
    } else {
        None
    }
}

/// `decode_tex(name)` -> (texture slot, sampler slot, handle type)
pub fn decode_tex(name: &str) -> Option<(Option<i64>, Option<i64>, i64)> {
    let m = RE_DEC_TEX.captures(name)?;
    let slots = m.get(1).map(|g| dec(g.as_str())).unwrap_or(-1);
    let h = hex(&m[2]);
    let tb = if slots >= 0 { slots & 0xffff } else { 2 };
    let sb = if slots >= 0 && (slots >> 16) != 0 {
        (slots >> 16) - 1
    } else {
        tb
    };
    let (o0, o1, ty) = handle_parts(h);
    let words: Vec<i64> = if ty != 0 { vec![o0, o1] } else { vec![o0] };
    if RE_IS_IMG.is_match(name) {
        let t = words
            .iter()
            .find(|&&x| {
                abi::C2_IMG <= x * 4
                    && x * 4 < abi::C2_IMG + abi::C2_ROW * abi::C2_IMG_SLOTS
                    && slot(x, abi::C2_IMG, tb).is_some()
            })
            .and_then(|&x| slot(x, abi::C2_IMG, tb));
        return Some((t, None, ty));
    }
    let tw: Vec<i64> = if ty == 2 || ty == 3 {
        vec![o0]
    } else {
        words.clone()
    };
    let mut t = tw
        .iter()
        .find(|&&x| slot(x, abi::C2_TEX, tb).is_some() && x * 4 < abi::C2_SEPTEX)
        .and_then(|&x| slot(x, abi::C2_TEX, tb));
    if t.is_none() {
        t = tw
            .iter()
            .find(|&&x| slot(x, abi::C2_SEPTEX, tb).is_some() && x * 4 < abi::C2_SAMP)
            .and_then(|&x| slot(x, abi::C2_SEPTEX, tb))
            .map(|s| abi::SEPTEX_SLOT0 + s);
    }
    let s = if ty != 3 {
        words
            .iter()
            .find(|&&x| x * 4 >= abi::C2_SAMP && slot(x, abi::C2_SAMP, sb).is_some())
            .and_then(|&x| slot(x, abi::C2_SAMP, sb))
    } else {
        None
    };
    Some((t, s, ty))
}

pub fn const_sampler(name: &str) -> Option<i64> {
    let m = RE_CONST_SAMP.captures(name)?;
    let h = hex(&m[2]);
    if h >> 28 == 3 {
        Some(handle_parts(h).1)
    } else {
        None
    }
}

/// the sampler half of `decode_ubo_tex`: `('id', ID)`, `('c2', byte offset)` or `(uniform slot, byte offset)`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UboS {
    Id(i64),
    C2(i64),
    At(i64, i64),
}

/// `decode_ubo_tex(name)` -> ((uniform slot, byte offset), sampler part)
pub fn decode_ubo_tex(name: &str) -> Option<((i64, i64), UboS)> {
    let m = RE_UBO_TEX.captures(name)?;
    let (slots, h) = (dec(&m[1]), hex(&m[2]));
    let tb = slots & 0xffff;
    let sb = if slots >> 16 != 0 {
        (slots >> 16) - 1
    } else {
        tb
    };
    let (o0, o1, ty) = handle_parts(h);
    if !(3..=16).contains(&tb)
        || !matches!(ty, 0 | 1 | 3)
        || (ty == 1 && sb != 2 && !(3..=16).contains(&sb))
    {
        return None;
    }
    let us = if ty == 3 {
        UboS::Id(o1)
    } else if ty != 0 && sb == 2 {
        UboS::C2(o1 * 4)
    } else if ty != 0 {
        UboS::At(sb - 3, o1 * 4)
    } else {
        UboS::At(tb - 3, o0 * 4)
    };
    Some(((tb - 3, o0 * 4), us))
}

pub fn decode_ubo_samp_id(name: &str) -> Option<(i64, i64)> {
    let m = RE_UBO_SAMP_ID.captures(name)?;
    let (slots, h) = (dec(&m[1]), hex(&m[2]));
    let sb = (slots >> 16) - 1;
    if h >> 28 == 2 && slots & 0xffff == 2 && (3..=16).contains(&sb) {
        Some((sb - 3, handle_parts(h).1 * 4))
    } else {
        None
    }
}

pub fn decode_ubo_img(name: &str) -> Option<(i64, i64)> {
    let m = RE_UBO_IMG.captures(name)?;
    let n = dec(&m[1]);
    if (3..=16).contains(&n) {
        Some((n - 3, i64_of(hex(&m[2]) * 4)))
    } else {
        None
    }
}

fn jint(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(|x| x.as_i64())
}

/// `v == x` for a json value and an int, as Python compares them (`True == 1`, `3.0 == 3`)
fn json_eq_int(v: &Value, x: i64) -> bool {
    match v {
        Value::Bool(b) => i64::from(*b) == x,
        Value::Number(n) => match n.as_i64() {
            Some(i) => i == x,
            None => n.as_f64().is_some_and(|f| f == x as f64),
        },
        _ => false,
    }
}

/// Python's truth value of a json value (`None` for a missing key)
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

pub fn buffer_image_bias(name: &str, res: &Value) -> Option<i64> {
    let m = RE_IMG_BIAS.captures(name)?;
    let slots = m.get(2).map(|g| dec(g.as_str()));
    let h = hex(&m[3]);
    let key = if &m[1] == "i" { "images" } else { "textures" };
    for i in res
        .get(key)
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let Some(tbx) = jint(i, "tbx") else { continue };
        if jint(i, "handle").map(i128::from) != Some(h) {
            continue;
        }
        let cb = jint(i, "cbuf_slot").unwrap_or(0);
        if match slots {
            Some(s) => cb == s,
            None => cb < 0,
        } {
            return Some(tbx);
        }
    }
    None
}

pub fn buffer_bias_entries(res: &Value) -> i64 {
    let mut n = 0;
    for k in ["textures", "images"] {
        for i in res.get(k).and_then(|v| v.as_array()).into_iter().flatten() {
            if let Some(tbx) = jint(i, "tbx") {
                // a wrapped 1D fetch (row_bias) takes a second entry per element: its logical width
                let rows = if truthy(i.get("row_bias")) { 2 } else { 1 };
                n = n.max(tbx + jint(i, "array").unwrap_or(1).max(1) * rows);
            }
        }
    }
    (n + 3) / 4 * 4
}

/// `c2_array(name, ty)` -> (c2 byte offset of element 0, the sampler word's byte offset or None)
pub fn c2_array(name: &str, ty: &str) -> Option<(i64, Option<i64>)> {
    let n = name.strip_suffix("Smplr").unwrap_or(name);
    let m = RE_C2ARR.captures(n)?;
    if !ty.starts_with("array<") {
        return None;
    }
    let (slots, h) = (dec(&m[1]), hex(&m[2]));
    let (o0, o1, _) = handle_parts(h);
    if h >> 28 == 0 && slots == 2 {
        return Some((o0 * 4, None));
    }
    if h >> 28 == 1 && slots & 0xffff == 2 && (slots >> 16) - 1 == 2 {
        return Some((o0 * 4, Some(o1 * 4)));
    }
    None
}

// ------------------------------------------------------------------------------------------ reflect
re!(
    RE_DESC_STRUCT,
    r"(?s)struct spvDescriptorSetBuffer0\s*\{(.*?)\};"
);
re!(
    RE_MEMBER,
    r"(?m)^\s*(.+?)\s+(\w+)\s*\[\[id\((\d+)\)(?:\s*,\s*\w+\(\d+\))*\]\];"
);
re!(RE_C_N, r"^[a-z]+_c(\d+)$");
re!(RE_S_N, r"^[a-z]+_s(\d+)$");
re!(
    RE_STORE_NOTE,
    r#"operation "(Store|Atomic\w*)" traced to no storage buffer"#
);

/// SSBO base of a stage (`SSBO.get(stage)`)
pub fn ssbo_base(stage: &str) -> Option<i64> {
    match stage {
        "vs" => Some(abi::C0_SSBO_VS),
        "cs" => Some(abi::C0_SSBO_CS),
        "gs" => Some(abi::C0_SSBO_GS),
        "ps" => Some(abi::C0_SSBO_PS),
        "ts" => Some(abi::C0_SSBO_TS),
        "ms" => Some(abi::C0_SSBO_MS),
        _ => None,
    }
}

fn helper_reg(nm: &str) -> Option<i64> {
    match nm {
        "vertex_output" => Some(0),
        "geometry_vb_output" => Some(1),
        "geometry_ib_output" => Some(2),
        "vb_info" => Some(3),
        "trb_data" => Some(4),
        "vb_input" => Some(1),
        _ => None,
    }
}

/// `reflect(key, stage, msl, res)` -> the bindings (ordered dicts), or the `ValueError` message
pub fn reflect(key: &str, stage: &str, msl: &str, res: &Value) -> Result<Vec<J>, String> {
    let body = RE_DESC_STRUCT
        .captures(msl)
        .map(|c| c.get(1).unwrap().as_str());
    let mut binds: Vec<J> = Vec::new();
    let members: Vec<regex::Captures> = RE_MEMBER.captures_iter(body.unwrap_or("")).collect();
    if let Some(b) = body
        && members.len() != b.matches("[[id(").count()
    {
        return Err(format!(
            "{key}: an argument buffer member the reflection cannot read"
        ));
    }
    for m in &members {
        let (ty, mut nm, aid) = (
            m[1].trim().to_string(),
            m[2].to_string(),
            m[3].parse::<i64>().unwrap_or(0),
        );
        let mut e = J::obj()
            .with("msl_name", nm.as_str())
            .with("msl_type", ty.as_str())
            .with("arg_id", aid);
        let pool_elem: Option<String> = if ty == format!("constant spvPool_{nm}*") {
            // `re.search(rf'struct spvPool_{re.escape(nm)}\s*\{{\s*uint n;...', msl)`: the literal head searched, the fixed tail matched after it (no pattern per member)
            super::rx::lit_then(
                msl,
                &format!("struct spvPool_{nm}"),
                r"\s*\{\s*uint n;\s*(?:uint bias;\s*)?ulong seen;\s*array<(.+), \d+> e;",
            )
            .map(|c| c[1].to_string())
        } else {
            None
        };
        if ty == "sampler" && nm.ends_with(&format!("Smplr{}", abi::BORDER_TWIN)) {
            nm.truncate(nm.len() - abi::BORDER_TWIN.len());
            let bit = binds.iter().filter(|b| b.has("border_bit")).count();
            e.set("border_bit", bit);
        }
        let smplr = nm.ends_with("Smplr");
        let base_nm = nm.strip_suffix("Smplr").unwrap_or(&nm).to_string();
        if nm == abi::BORDER_MEMBER {
            e.set("kind", abi::KIND_BORDER);
        } else if let Some(r) = helper_reg(&nm) {
            e.set("kind", abi::KIND_GEOMETRY);
            e.set("reg", r);
        } else if let (Some(el), false) = (&pool_elem, nm.ends_with("_cb0_40000000")) {
            let (ut, us) =
                decode_ubo_tex(nm.strip_suffix("Smplr").unwrap_or(&nm)).ok_or_else(|| {
                    "pool_arrays made this pool for a name it does not decode".to_string()
                })?;
            e.set(
                "kind",
                if el == "sampler" {
                    abi::KIND_SAMP_HTAB
                } else {
                    abi::KIND_TEX_HTAB
                },
            );
            e.set("elem", el.as_str());
            e.set("reg", ut.0);
            e.set("off", ut.1);
            if let UboS::C2(off) = us {
                e.set("soff", off);
                e.set("sreg", abi::SREG_C2);
            }
        } else if let Some(el) = &pool_elem {
            e.set(
                "kind",
                if el == "sampler" {
                    abi::KIND_SAMP_POOL
                } else {
                    abi::KIND_TEX_POOL
                },
            );
            e.set("elem", el.as_str());
        } else if nm == "support_buffer" {
            e.set("kind", abi::KIND_SUPPORT);
        } else if let Some(c) = RE_C_N.captures(&nm) {
            let n = dec(&c[1]);
            if (3..=16).contains(&n) {
                let fixed = res
                    .get("cbufs")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.iter().find(|b| jint(b, "slot") == Some(n)))
                    .map(|b| jint(b, "fixed").unwrap_or(0))
                    .unwrap_or(0);
                e.set("kind", abi::KIND_CBUF);
                e.set("reg", n - 3);
                e.set("fixed", fixed);
            } else if n == abi::CBSIZE_BANK {
                e.set("kind", abi::KIND_CBSIZE);
            } else if n == abi::TBIAS_BANK {
                e.set("kind", abi::KIND_TBIAS);
                e.set("n", buffer_bias_entries(res));
            } else if n == abi::SBLEN_BANK {
                e.set("kind", abi::KIND_SBLEN);
            } else {
                e.set(
                    "kind",
                    if n == 1 {
                        abi::KIND_C1
                    } else {
                        abi::KIND_DRVCB
                    },
                );
                e.set("bank", n);
            }
        } else if let Some(c) = RE_S_N.captures(&nm) {
            e.set("kind", abi::KIND_SBUF);
            e.set("index", dec(&c[1]));
        } else if let (true, Some(si)) = (
            ty == "sampler" && smplr,
            if smplr {
                decode_ubo_samp_id(&base_nm)
            } else {
                None
            },
        ) {
            e.set("kind", abi::KIND_SAMP_UBO);
            e.set("reg", si.0);
            e.set("off", si.1);
            e.set("raw_id", true);
        } else if let Some((ut, us)) = decode_ubo_tex(&base_nm) {
            let other = match us {
                UboS::Id(_) | UboS::C2(_) => false,
                UboS::At(a, b) => (a, b) != ut,
            };
            if smplr && matches!(us, UboS::C2(_)) {
                let UboS::C2(soff) = us else { unreachable!() };
                e.set("kind", abi::KIND_SAMP_UBO);
                e.set("reg", ut.0);
                e.set("off", ut.1);
                e.set("soff", soff);
                e.set("sreg", abi::SREG_C2);
            } else if smplr {
                match us {
                    UboS::Id(id) => {
                        e.set("kind", abi::KIND_SAMP_ID);
                        e.set("reg", id);
                    }
                    UboS::At(a, b) => {
                        e.set("kind", abi::KIND_SAMP_UBO);
                        e.set("reg", a);
                        e.set("off", b);
                        if other {
                            e.set("soff", ut.1);
                            e.set("sreg", ut.0);
                        }
                    }
                    UboS::C2(_) => unreachable!(),
                }
            } else if ty.contains("texture_buffer") {
                e.set("kind", abi::KIND_TBUF_UBO);
                e.set("reg", ut.0);
                e.set("off", ut.1);
                if other && let UboS::At(a, b) = us {
                    e.set("soff", b);
                    e.set("sreg", a);
                }
                if !ty.starts_with("array<")
                    && let Some(x) = buffer_image_bias(&nm, res)
                {
                    e.set("tbx", x);
                }
            } else {
                e.set("kind", abi::KIND_TEX_UBO);
                e.set("reg", ut.0);
                e.set("off", ut.1);
                if other && let UboS::At(a, b) = us {
                    e.set("soff", b);
                    e.set("sreg", a);
                }
            }
        } else if let Some(ui) = decode_ubo_img(&nm) {
            e.set("kind", abi::KIND_IMG_UBO);
            e.set("reg", ui.0);
            e.set("off", ui.1);
            if let Some(x) = buffer_image_bias(&nm, res) {
                e.set("tbx", x);
            }
            if has_word(msl, &format!("{nm}.atomic_")) {
                e.set("atomic", true);
            }
        } else if let Some((off, soff)) = c2_array(&nm, &ty) {
            e.set(
                "kind",
                if smplr {
                    abi::KIND_SAMP_C2A
                } else {
                    abi::KIND_TEX_C2A
                },
            );
            e.set("off", off);
            if let Some(s) = soff {
                e.set("soff", s);
            }
            if !smplr
                && ty.contains("texture_buffer")
                && let Some(x) = buffer_image_bias(&nm, res)
            {
                e.set("tbx", x);
            }
        } else if smplr {
            let (t, s, tyc) = decode_tex(&base_nm)
                .ok_or_else(|| format!("{key}: a sampler whose name does not decode ({nm})"))?;
            if tyc == 3 {
                e.set("kind", abi::KIND_SAMP_ID);
                e.set("reg", const_sampler(&base_nm));
            } else if tyc != 0 {
                e.set("kind", abi::KIND_SAMP);
                e.set("reg", s);
            } else {
                e.set("kind", abi::KIND_SAMP_TEX);
                e.set("reg", t);
            }
        } else {
            let d = decode_tex(&nm);
            match d {
                None => e.set("kind", abi::KIND_UNKNOWN),
                Some((t, _, _)) => {
                    let kind = if RE_IS_IMG.is_match(&nm) {
                        abi::KIND_IMG
                    } else if ty.contains("texture_buffer") {
                        abi::KIND_TBUF
                    } else {
                        abi::KIND_TEX
                    };
                    e.set("kind", kind);
                    e.set("reg", t);
                }
            }
            let kind = e
                .get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or("")
                .to_string();
            if kind == abi::KIND_IMG && has_word(msl, &format!("{nm}.atomic_")) {
                e.set("atomic", true);
            }
            if (kind == abi::KIND_IMG || kind == abi::KIND_TBUF)
                && let Some(x) = buffer_image_bias(&nm, res)
            {
                e.set("tbx", x);
            }
            if kind == abi::KIND_TEX
                && let Some(reg) = e.get("reg").and_then(|r| r.as_i64())
                && res
                    .get("fetch1d")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().any(|x| x.as_i64() == Some(reg)))
                    .unwrap_or(false)
            {
                e.set("fetch1d", true);
                if has_word(msl, &format!("{nm}.get_width(")) {
                    e.set("fetch1d_size", true);
                }
            }
        }
        // a texture whose offset table entry is a wrapped 1D fetch's (row_bias): bound as rows, its logical width in the table's second group
        let texture = matches!(
            e.get("kind").and_then(|k| k.as_str()),
            Some(abi::KIND_TEX | abi::KIND_TEX_UBO | abi::KIND_TEX_C2A)
        );
        if texture
            && let Some(x) = buffer_image_bias(&nm, res)
            && res
                .get("textures")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .any(|t| {
                    truthy(t.get("row_bias")) && t.get("tbx").is_some_and(|v| json_eq_int(v, x))
                })
        {
            e.set("tbx", x);
            e.set("fetch1d", true);
        }
        binds.push(e);
    }
    // storage buffers: SPIR-V binding number -> c0 offset -> NVN storage buffer slot
    let base = ssbo_base(stage);
    let mut sb_sorted: Vec<&Value> = res
        .get("sbufs")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    sb_sorted.sort_by_key(|b| jint(b, "binding").unwrap_or(0));
    let notes: Vec<&str> = res
        .get("notes")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default();
    for e in binds.iter_mut() {
        if e.get("kind").and_then(|k| k.as_str()) != Some(abi::KIND_SBUF) {
            continue;
        }
        let index = e.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
        let b = if (index as usize) < sb_sorted.len() && index >= 0 {
            Some(sb_sorted[index as usize])
        } else {
            None
        };
        let cb_slot = b.and_then(|b| jint(b, "cb_slot"));
        let cb_off = b.and_then(|b| jint(b, "cb_offset"));
        let flags = b.and_then(|b| jint(b, "flags")).unwrap_or(0);
        let off = if cb_slot == Some(0) {
            cb_off.map(|o| o * 4)
        } else {
            None
        };
        if cb_slot == Some(abi::GMEM_MAP_CB_SLOT) {
            if cb_off == Some(abi::GMEM_LOG_CB_OFFSET) {
                e.set("kind", abi::KIND_GMEM_LOG);
                e.set("rw", true);
                continue;
            }
            let rw = notes.iter().any(|n| RE_STORE_NOTE.is_match(n));
            e.set("kind", abi::KIND_GMEM_MAP);
            e.set("rw", rw);
            e.set("pointers", gptr_sites(stage, res)?);
            continue;
        }
        if stage == "cs"
            && off
                .map(|o| {
                    (abi::C0_CS_UBO..abi::C0_CS_UBO + abi::C0_ROW * abi::C0_CS_UBO_SLOTS)
                        .contains(&o)
                        && (o - abi::C0_CS_UBO) % abi::C0_ROW == 0
                })
                .unwrap_or(false)
        {
            e.set("kind", abi::KIND_UBO_PTR);
            e.set("reg", (off.unwrap() - abi::C0_CS_UBO) / abi::C0_ROW);
        } else if cb_slot == Some(abi::PUSH_BANK) {
            e.set("kind", abi::KIND_SBUF_PUSH);
            e.set("off", cb_off.unwrap_or(0) * 4);
        } else if cb_slot.map(|s| s >= 3).unwrap_or(false) {
            e.set("kind", abi::KIND_SBUF_UBO);
            e.set("reg", cb_slot.unwrap() - 3);
            e.set("off", cb_off.unwrap_or(0) * 4);
        } else {
            let reg = match (off, base) {
                (Some(o), Some(bs))
                    if 0 <= o - bs
                        && o - bs < abi::C0_ROW * abi::C0_SSBO_SLOTS
                        && (o - bs) % abi::C0_ROW == 0 =>
                {
                    J::Int((o - bs) / abi::C0_ROW)
                }
                _ => J::Null,
            };
            e.set("reg", reg);
        }
        e.set("rw", b.is_some() && flags & 1 != 0);
        if b.is_some() && flags & 2 != 0 {
            let kind = e.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            let slot = b.and_then(|b| jint(b, "slot")).unwrap_or(-1);
            if !(kind == abi::KIND_SBUF_UBO || kind == abi::KIND_SBUF_PUSH)
                || !(0..abi::SBLEN_SLOTS).contains(&slot)
            {
                return Err(format!(
                    "{key}: storage buffer {} is bounded but is a {kind} binding, slot {slot}",
                    e.get("msl_name").and_then(|n| n.as_str()).unwrap_or("")
                ));
            }
            e.set("sbx", slot);
        }
    }
    if binds.iter().any(|e| e.has("sbx"))
        && !binds
            .iter()
            .any(|e| e.get("kind").and_then(|k| k.as_str()) == Some(abi::KIND_SBLEN))
    {
        return Err(format!(
            "{key}: bounded storage buffers without their length table"
        ));
    }
    Ok(binds)
}

// ------------------------------------------------------------------------------------------ gptr_sites
re!(
    RE_GPTR_SITE,
    r"\Atranslator: gptr site ([0-9]+): c([0-9]+)\[([0-9]+)\] \+([0-9]+)\n?\z"
);

/// A decimal digit string as Python's `int` reads it, for ordering and printing: no leading zeros
fn int_text(d: &str) -> String {
    let t = d.trim_start_matches('0');
    if t.is_empty() { "0" } else { t }.to_string()
}

/// `gptr_sites(stage, res)`: the fixed pointer words in NVN storage slots the translator found (its notes `translator: gptr site k: c<slot>[<word>] +<byte>`),
/// `[slot, byte]` in site order, the sites in no storage slot of the stage left out; Err (the `ValueError` message) when the sites are not 0..n-1 within `GPTR_MAX`
pub fn gptr_sites(stage: &str, res: &Value) -> Result<J, String> {
    // site number (as int text) -> [reg, byte]; ordered as Python orders the ints: by length, then by text
    let mut sites: Vec<(String, i64, i64)> = Vec::new();
    let base = ssbo_base(stage);
    for note in res
        .get("notes")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|n| n.as_str())
    {
        let Some(m) = RE_GPTR_SITE.captures(note) else {
            continue;
        };
        let k = int_text(&m[1]);
        let num = |i: usize| m[i].parse::<i64>().ok();
        let (cb_slot, cb_word, byte) = (num(2), num(3), num(4));
        // a word past i64 is past every storage slot
        let off = cb_word.and_then(|w| w.checked_mul(4));
        let reg = match (cb_slot, off, base) {
            (Some(0), Some(o), Some(b))
                if 0 <= o - b
                    && o - b < abi::C0_ROW * abi::C0_SSBO_SLOTS
                    && (o - b) % abi::C0_ROW == 0 =>
            {
                (o - b) / abi::C0_ROW
            }
            _ => -1,
        };
        let byte = byte.unwrap_or(i64::MAX);
        match sites.iter_mut().find(|s| s.0 == k) {
            Some(s) => {
                s.1 = reg;
                s.2 = byte;
            }
            None => sites.push((k, reg, byte)),
        }
    }
    sites.sort_by(|a, b| (a.0.len(), &a.0).cmp(&(b.0.len(), &b.0)));
    let in_order = sites.iter().enumerate().all(|(i, s)| s.0 == i.to_string());
    if !in_order || sites.len() as i64 > abi::GPTR_MAX {
        let keys: Vec<&str> = sites.iter().map(|s| s.0.as_str()).collect();
        return Err(format!(
            "gptr sites [{}] are not 0..n-1 within {}",
            keys.join(", "),
            abi::GPTR_MAX
        ));
    }
    Ok(J::Arr(
        sites
            .into_iter()
            .filter(|s| s.1 >= 0)
            .map(|s| J::Arr(vec![J::Int(s.1), J::Int(s.2)]))
            .collect(),
    ))
}

// ------------------------------------------------------------------------------------------ drv_reads / unfillable
re!(
    RE_INDEX_DEF,
    r"(?m)^\s*(?:int|uint)\s+(_\d+)\s*=\s*([^;\n]+);"
);
re!(RE_INDEX_SET, r"(?m)^\s*(_\d+)\s*=");
re!(
    RE_FLOAT_DEF,
    r"(?m)^\s*float\s+(_\d+)\s*=\s*as_type<float>\(([\w.]+)\);"
);
re!(RE_FP0, r"fp_c0\)\._m0\[([^\]]+)\]");
re!(RE_FP2, r"fp_c2\)\._m0\[([^\]]+)\]");
re!(RE_CAST, r"\b(?:as_type<\s*u?int\s*>|u?int)\s*\(");
re!(RE_NUM_U, r"\b(\d+)u\b");

#[derive(Debug)]
enum Node {
    Const(i64),
    Name(String),
    Call(Box<Node>),
    Bin(u8, Box<Node>, Box<Node>),
    Bad,
}

struct Parser {
    t: Vec<Tk>,
    i: usize,
}
#[derive(Debug, Clone, PartialEq)]
enum Tk {
    Num(i64),
    Name(String),
    Op(&'static str),
}

fn tokenize(e: &str) -> Option<Vec<Tk>> {
    if e.starts_with(|c: char| c.is_whitespace()) {
        return None;
    } // ast.parse(mode='eval') raises IndentationError
    let b = e.as_bytes();
    let (mut i, mut depth, mut out) = (0usize, 0i32, Vec::new());
    while i < b.len() {
        let c = b[i];
        if c == b' ' || c == b'\t' || c == 0x0c {
            i += 1;
            continue;
        }
        if c == b'\n' || c == b'\r' {
            if depth > 0 {
                i += 1;
                continue;
            }
            if b[i..]
                .iter()
                .all(|x| matches!(x, b' ' | b'\t' | b'\n' | b'\r'))
            {
                break;
            }
            return None;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            if c == b'0' && i + 1 < b.len() && (b[i + 1] == b'x' || b[i + 1] == b'X') {
                j = i + 2;
                while j < b.len() && b[j].is_ascii_hexdigit() {
                    j += 1;
                }
                if j == i + 2 || (j < b.len() && (is_word(b[j]) || b[j] == b'.')) {
                    return None;
                }
                out.push(Tk::Num(i64::from_str_radix(&e[i + 2..j], 16).ok()?));
                i = j;
                continue;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j < b.len() && (is_word(b[j]) || b[j] == b'.') {
                return None;
            } // a float, a suffix, an identifier glued to a number: not an integer constant
            let s = &e[i..j];
            if s.len() > 1 && s.starts_with('0') && s.bytes().any(|d| d != b'0') {
                return None;
            }
            out.push(Tk::Num(s.parse().ok()?));
            i = j;
            continue;
        }
        if c == b'_' || c.is_ascii_alphabetic() {
            let mut j = i;
            while j < b.len() && is_word(b[j]) {
                j += 1;
            }
            out.push(Tk::Name(e[i..j].to_string()));
            i = j;
            continue;
        }
        let two = e.get(i..i + 2).unwrap_or("");
        let tk = match two {
            "<<" => Some("<<"),
            ">>" => Some(">>"),
            "//" => Some("//"),
            "**" => Some("**"),
            _ => None,
        };
        if let Some(t) = tk {
            out.push(Tk::Op(t));
            i += 2;
            continue;
        }
        let t = match c {
            b'+' => "+",
            b'-' => "-",
            b'*' => "*",
            b'/' => "/",
            b'%' => "%",
            b'@' => "@",
            b'&' => "&",
            b'|' => "|",
            b'^' => "^",
            b'~' => "~",
            b',' => ",",
            b'(' => {
                depth += 1;
                "("
            }
            b')' => {
                depth -= 1;
                ")"
            }
            _ => return None,
        };
        out.push(Tk::Op(t));
        i += 1;
    }
    Some(out)
}

const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

impl Parser {
    fn peek(&self) -> Option<&Tk> {
        self.t.get(self.i)
    }
    fn op(&self, o: &str) -> bool {
        matches!(self.peek(), Some(Tk::Op(x)) if *x == o)
    }
    fn bin(&mut self, ops: &[&'static str], next: fn(&mut Parser) -> Option<Node>) -> Option<Node> {
        let mut l = next(self)?;
        while let Some(Tk::Op(o)) = self.peek().cloned() {
            if !ops.contains(&o) {
                break;
            }
            self.i += 1;
            let r = next(self)?;
            let code = match o {
                "|" => b'|',
                "^" => b'^',
                "&" => b'&',
                "<<" => b'<',
                ">>" => b'>',
                "+" => b'+',
                "-" => b'-',
                "*" => b'*',
                _ => 0,
            };
            l = if code == 0 {
                Node::Bad
            } else {
                Node::Bin(code, Box::new(l), Box::new(r))
            };
        }
        Some(l)
    }
    fn or_(&mut self) -> Option<Node> {
        self.bin(&["|"], Parser::xor_)
    }
    fn xor_(&mut self) -> Option<Node> {
        self.bin(&["^"], Parser::and_)
    }
    fn and_(&mut self) -> Option<Node> {
        self.bin(&["&"], Parser::shift)
    }
    fn shift(&mut self) -> Option<Node> {
        self.bin(&["<<", ">>"], Parser::arith)
    }
    fn arith(&mut self) -> Option<Node> {
        self.bin(&["+", "-"], Parser::term)
    }
    fn term(&mut self) -> Option<Node> {
        self.bin(&["*", "/", "//", "%", "@"], Parser::factor)
    }
    fn factor(&mut self) -> Option<Node> {
        if self.op("+") || self.op("-") || self.op("~") {
            self.i += 1;
            self.factor()?;
            return Some(Node::Bad);
        }
        self.power()
    }
    fn power(&mut self) -> Option<Node> {
        let l = self.primary()?;
        if self.op("**") {
            self.i += 1;
            self.factor()?;
            return Some(Node::Bad);
        }
        Some(l)
    }
    fn primary(&mut self) -> Option<Node> {
        match self.peek()?.clone() {
            Tk::Num(n) => {
                self.i += 1;
                Some(Node::Const(n))
            }
            Tk::Name(nm) => {
                if KEYWORDS.contains(&nm.as_str()) {
                    return None;
                }
                self.i += 1;
                if self.op("(") {
                    self.i += 1;
                    let mut args = 0;
                    let mut first: Option<Node> = None;
                    loop {
                        if self.op(")") {
                            self.i += 1;
                            break;
                        }
                        let a = self.or_()?;
                        args += 1;
                        if first.is_none() {
                            first = Some(a);
                        }
                        if self.op(",") {
                            self.i += 1;
                            continue;
                        }
                        if self.op(")") {
                            self.i += 1;
                            break;
                        }
                        return None;
                    }
                    if nm == "_c" && args == 1 {
                        return Some(Node::Call(Box::new(first.unwrap())));
                    }
                    return Some(Node::Bad);
                }
                Some(Node::Name(nm))
            }
            Tk::Op("(") => {
                self.i += 1;
                let e = self.or_()?;
                if self.op(",") {
                    // a tuple
                    while self.op(",") {
                        self.i += 1;
                        if self.op(")") {
                            break;
                        }
                        self.or_()?;
                    }
                    if !self.op(")") {
                        return None;
                    }
                    self.i += 1;
                    return Some(Node::Bad);
                }
                if !self.op(")") {
                    return None;
                }
                self.i += 1;
                Some(e)
            }
            _ => None,
        }
    }
}

struct IndexCtx<'a> {
    defs: HashMap<&'a str, Vec<&'a str>>,
    sets: BTreeSet<&'a str>,
    float_defs: HashMap<&'a str, &'a str>,
}

fn parse_index(cx: &IndexCtx, e: &str, depth: u32) -> Option<(i64, i64)> {
    let e1 = RE_CAST.replace_all(e, "_c(");
    let e2 = RE_NUM_U.replace_all(&e1, "$1");
    let toks = tokenize(&e2)?;
    let mut p = Parser { t: toks, i: 0 };
    let tree = p.or_()?;
    if p.i != p.t.len() {
        return None;
    }
    ev(cx, &tree, depth)
}

fn ev(cx: &IndexCtx, n: &Node, depth: u32) -> Option<(i64, i64)> {
    if depth > 64 {
        return None;
    }
    match n {
        Node::Const(v) => Some((*v, *v)),
        Node::Name(id) => {
            if id == "gl_SampleID" {
                return Some((0, abi::C0_SAMPLE_BYTES - 1));
            }
            if let Some(f) = cx.float_defs.get(id.as_str())
                && *f == "gl_SampleID"
                && !cx.sets.contains(id.as_str())
            {
                return Some((0, abi::C0_SAMPLE_BYTES - 1));
            }
            let d = cx.defs.get(id.as_str())?;
            if d.len() != 1 || cx.sets.contains(id.as_str()) {
                return None;
            }
            parse_index(cx, d[0], depth + 1)
        }
        Node::Call(a) => ev(cx, a, depth + 1),
        Node::Bin(op, l, r) => {
            let (a, b) = (ev(cx, l, depth + 1), ev(cx, r, depth + 1));
            let (a, b) = (a?, b?);
            let res = match op {
                b'+' => (a.0.checked_add(b.0)?, a.1.checked_add(b.1)?),
                b'-' => (a.0.checked_sub(b.1)?, a.1.checked_sub(b.0)?),
                b'*' => (a.0.checked_mul(b.0)?, a.1.checked_mul(b.1)?),
                b'>' if b.0 == b.1 && b.0 < 32 && b.0 >= 0 => (a.0 >> b.0, a.1 >> b.0),
                b'<' if b.0 == b.1 && b.0 < 32 && b.0 >= 0 => {
                    (a.0.checked_shl(b.0 as u32)?, a.1.checked_shl(b.0 as u32)?)
                }
                b'&' => (0, a.1.min(b.1)),
                // `(1 << max(..).bit_length()) - 1`: past 31 bits it fails the bound below anyway (and `1 << 63` - 1 overflowed an i64)
                b'|' | b'^' => {
                    let bits = 64 - a.1.max(b.1).leading_zeros();
                    if bits > 31 {
                        return None;
                    }
                    (0, (1i64 << bits) - 1)
                }
                _ => return None,
            };
            if 0 <= res.0 && res.0 <= res.1 && res.1 < (1 << 31) {
                Some(res)
            } else {
                None
            }
        }
        Node::Bad => None,
    }
}

pub fn index_range(src: &str, expr: &str) -> Option<(i64, i64)> {
    let sets: BTreeSet<&str> = RE_INDEX_SET
        .captures_iter(src)
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    let mut defs: HashMap<&str, Vec<&str>> = HashMap::new();
    for c in RE_INDEX_DEF.captures_iter(src) {
        defs.entry(c.get(1).unwrap().as_str())
            .or_default()
            .push(c.get(2).unwrap().as_str());
    }
    let mut float_defs: HashMap<&str, &str> = HashMap::new();
    for c in RE_FLOAT_DEF.captures_iter(src) {
        float_defs.insert(c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str());
    }
    parse_index(
        &IndexCtx {
            defs,
            sets,
            float_defs,
        },
        expr,
        0,
    )
}

/// `index * 16` clamped to the i64 range (the reflection json's integers)
fn row_offset(index: i128) -> i64 {
    index
        .saturating_mul(16)
        .clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// `drv_reads(src)`: {"0": {vec4_offsets, dynamic}, "2": {...}} (a bank without a read is absent)
pub fn drv_reads(src: &str) -> J {
    let mut out = J::obj();
    for (bank, re) in [(0, &*RE_FP0), (2, &*RE_FP2)] {
        let (mut offs, mut dynamic) = (BTreeSet::<i64>::new(), false);
        for m in re.captures_iter(src) {
            let g = m.get(1).unwrap().as_str();
            let idx = g.trim_end_matches('u');
            if !idx.is_empty() && idx.bytes().all(|c| c.is_ascii_digit()) {
                // `int(idx) * 16`, unbounded in Python: an offset past i64 is kept as i64::MAX (outside every table, so `unfillable` flags it as the Python
                // does; only the number written differs). It was recorded as a dynamic index, which `unfillable` does not flag in c2.
                offs.insert(row_offset(idx.parse::<i128>().unwrap_or(i128::MAX)));
                continue;
            }
            match index_range(src, g) {
                Some(r) if r.1 - r.0 < 16 => {
                    // `range(lo * 16, hi * 16 + 16, 16)`: a constant index is not bounded by index_range, so the products are taken in i128
                    let (mut o, end) = (r.0 as i128 * 16, r.1 as i128 * 16 + 16);
                    while o < end {
                        offs.insert(row_offset(o / 16));
                        o += 16;
                    }
                }
                _ => dynamic = true,
            }
        }
        if !offs.is_empty() || dynamic {
            out.set(
                &bank.to_string(),
                J::obj()
                    .with(
                        "vec4_offsets",
                        J::Arr(offs.into_iter().map(J::Int).collect()),
                    )
                    .with("dynamic", dynamic),
            );
        }
    }
    out
}

fn drv_fill(bank: i64) -> Vec<(i64, i64)> {
    let row = abi::C0_ROW * abi::C0_SSBO_SLOTS;
    match bank {
        0 => {
            let mut v = vec![
                abi::C0_SSBO_VS,
                abi::C0_SSBO_CS,
                abi::C0_SSBO_GS,
                abi::C0_SSBO_PS,
            ];
            v.sort();
            v.into_iter().map(|b| (b, b + row)).collect()
        }
        _ => vec![
            (abi::C2_TEX, abi::C2_SEPTEX),
            (abi::C2_IMG, abi::C2_IMG + abi::C2_ROW * abi::C2_IMG_SLOTS),
            (abi::C2_SEPTEX, abi::C2_SAMP),
            (abi::C2_SAMP, abi::C2_SAMP + abi::C2_ROW * abi::C2_SEP_SLOTS),
        ],
    }
}

pub fn unfillable(dr: &J, stage: &str) -> Vec<String> {
    let mut bad = Vec::new();
    let J::Obj(banks) = dr else { return bad };
    for (bank, d) in banks {
        let bn: i64 = bank.parse().unwrap_or(0);
        let mut fill = drv_fill(bn);
        if bn == 0 && stage == "ps" {
            fill.push((
                abi::C0_SAMPLE_TABLE,
                abi::C0_SAMPLE_TABLE + abi::C0_SAMPLE_BYTES,
            ));
            fill.push((abi::C0_SAMPLE_INFO, abi::C0_SAMPLE_INFO + abi::C0_ROW));
        }
        if bn == 0 && (stage == "ts" || stage == "ms") {
            let b = ssbo_base(stage).unwrap_or(0);
            fill.push((b, b + abi::C0_ROW * abi::C0_SSBO_SLOTS));
        }
        if bn == 0 && stage == "cs" {
            fill.push((
                abi::C0_CS_UBO,
                abi::C0_CS_UBO + abi::C0_ROW * abi::C0_CS_UBO_SLOTS,
            ));
        }
        let offs: Vec<i64> = d
            .get("vec4_offsets")
            .and_then(|v| v.as_arr())
            .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
            .unwrap_or_default();
        let dynamic = d.get("dynamic").and_then(|v| v.as_bool()).unwrap_or(false);
        if dynamic && bn == 0 && !(stage == "ps" && offs.contains(&abi::C0_SAMPLE_INFO)) {
            bad.push(format!("c{bank} dynamic index"));
        }
        for o in offs {
            if !fill.iter().any(|&(a, b)| o < b && a < o.saturating_add(16)) {
                bad.push(format!("c{bank}+{o:#x}"));
            }
        }
    }
    bad
}

/// all `user(locnN)` numbers of an MSL text (`re.findall(r'\[\[user\(locn(\d+)\)', text)`)
pub fn user_locations(msl: &str) -> BTreeSet<u32> {
    re!(RE_LOCN, r"\[\[user\(locn(\d+)\)");
    RE_LOCN
        .captures_iter(msl)
        .filter_map(|c| c[1].parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_constant_driver_index_past_i64_is_still_an_unfillable_read() {
        // all digits, past i64: Python's offset is unbounded and outside every table (this was recorded as a dynamic index, which c2 does not flag)
        let dr = drv_reads("float4 v = (*fp_c2)._m0[100000000000000000000];\n");
        assert_eq!(unfillable(&dr, "vs"), vec!["c2+0x7fffffffffffffff"]);
        // a parenthesized constant goes through index_range: 2^59 * 16 overflowed an i64
        let dr = drv_reads("float4 v = (*fp_c2)._m0[(576460752303423488)];\n");
        assert_eq!(unfillable(&dr, "vs"), vec!["c2+0x7fffffffffffffff"]);
        // an index that ORs a 63-bit constant: (1 << 63) - 1 overflowed; it is no bounded range (dynamic)
        assert_eq!(index_range("", "(9223372036854775807 | 1)"), None);
        assert_eq!(index_range("", "(5 | 2)"), Some((0, 7)));
    }

    #[test]
    fn a_handle_past_i64_keeps_its_type_and_offsets() {
        // int('8000000030004005', 16): type h >> 28 = 0x800000003, o0 = 5, o1 = 1 (an i64 parse gave 0: type 0, offsets 0)
        let h = hex("8000000030004005");
        assert_eq!(handle_parts(h), (5, 1, 0x8_0000_0003));
        assert_eq!(hex("0030004005"), 0x30004005);
        // 40 digits: the low bits are kept, the type is still large (never 0..3)
        let h = hex(&format!("1{}", "0".repeat(39)));
        assert_eq!(handle_parts(h).0, 0);
        assert!(h >> 28 > 3);
        // a type-3 handle past 2^63 is not a constant sampler (Python: h >> 28 != 3)
        assert_eq!(const_sampler("fp_t_tcb_8000000030004005"), None);
        assert_eq!(const_sampler("fp_t_tcb_30004005"), Some(1));
    }
}
