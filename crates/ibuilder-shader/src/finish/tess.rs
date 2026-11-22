//! `dmsh_tess.py`: tessellation without a Metal tessellator. A hull shader (`hs`) is rewritten as a plain kernel reading its control points from the capture records, a domain
//! shader (`ds`) as a plain vertex function on the host's grid of points (`TESS_MSL`, shared with the host's expansion kernel). Both are rewritten from SPIRV-Cross's MSL and the
//! SPIR-V decorations (`spirv_io`); an unsupported shape is a `ValueError` in the Python (here an `Err(message)`): `finish` then leaves the source as it was and records
//! `{"unsupported": message[:160]}` in the reflection.
//!
//! The functions return the Python's exact `ValueError` messages (the reflection JSON carries them).
use super::pycompat::{py_int, splitlines, strip};
use super::pyjson::J;
use super::tess_text::TESS_MSL;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

pub const CP_SLOTS: usize = 33;
pub const PATCH_SLOTS: usize = 32;
pub const CAPTURE_WORDS: usize = 144;
/// domain (vertex stage) buffer indices
pub const CP_BUFFER: u32 = 14;
pub const INFO_BUFFER: u32 = 15;
/// hull (compute) buffer indices
pub const INDEX_BUFFER: u32 = 24;
pub const CAPTURE_BUFFER: u32 = 25;

/// `{'mode', 'spacing', 'winding'}` of the domain shader's control section
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode {
    pub mode: String,
    pub spacing: String,
    pub winding: String,
}

impl Mode {
    /// from the job's `tess_mode` object (None when it is absent or null)
    pub fn from_json(v: &serde_json::Value) -> Option<Mode> {
        let o = v.as_object()?;
        let g = |k: &str| o.get(k).and_then(|x| x.as_str()).map(|s| s.to_string());
        Some(Mode {
            mode: g("mode")?,
            spacing: g("spacing")?,
            winding: g("winding")?,
        })
    }
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"mode": self.mode, "spacing": self.spacing, "winding": self.winding})
    }
}

/// `mode_bits(mode)`: bit 0 triangles, bits 1-2 the spacing (0 equal, 1 fractional_odd, 2 fractional_even), bit 3 clockwise
pub fn mode_bits(m: &Mode) -> Result<u32, String> {
    let sp = match m.spacing.as_str() {
        "equal" => 0,
        "fractional_odd" => 1,
        "fractional_even" => 2,
        other => return Err(format!("KeyError: '{other}'")),
    };
    Ok((m.mode == "triangles") as u32 | sp << 1 | ((m.winding == "cw") as u32) << 3)
}

/// `control_mode(ctl)`: a domain shader's own tessellation mode from its control section (+0x738 primitive mode, +0x73c spacing, +0x744 vertex order, as OpenGL enums)
pub fn control_mode(ctl: &[u8]) -> Option<Mode> {
    if ctl.len() < 0x748 {
        return None;
    }
    let u = |o: usize| u32::from_le_bytes([ctl[o], ctl[o + 1], ctl[o + 2], ctl[o + 3]]);
    let mode = match u(0x738) {
        0x0004 => "triangles",
        0x0007 => "quads",
        0x8E7A => "isolines",
        _ => return None,
    };
    let spacing = match u(0x73c) {
        0x0202 => "equal",
        0x8E7B => "fractional_odd",
        0x8E7C => "fractional_even",
        _ => return None,
    };
    let order = match u(0x744) {
        0x0900 => "cw",
        0x0901 => "ccw",
        _ => return None,
    };
    Some(Mode {
        mode: mode.into(),
        spacing: spacing.into(),
        winding: order.into(),
    })
}

/// one interface variable of the SPIR-V (`spirv_io`)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Io {
    /// true: Input storage class, false: Output
    pub input: bool,
    pub location: Option<u32>,
    pub patch: bool,
    pub builtin: Option<u32>,
}

/// An error message that starts with this character is not a `ValueError` of the Python but another exception (`struct.error`): the Python's `finish` does not catch it, its build stops
pub const CRASH: char = '\u{1}';

/// the message without the [`CRASH`] marker
pub fn plain(msg: &str) -> &str {
    msg.strip_prefix(CRASH).unwrap_or(msg)
}

/// `spirv_io(path)`, from the file's bytes: {variable id: Io} for every Input / Output variable. A file whose size is not a multiple of four bytes is a `struct.error` in the Python
/// (`struct.unpack('<%dI' % (len(data) // 4), data)`), a [`CRASH`] here.
pub fn spirv_io(data: &[u8]) -> Result<BTreeMap<u32, Io>, String> {
    let nw = data.len() / 4;
    if !data.len().is_multiple_of(4) {
        return Err(format!(
            "{CRASH}struct.error: unpack requires a buffer of {} bytes",
            nw * 4
        ));
    }
    let w = |i: usize| {
        u32::from_le_bytes([
            data[4 * i],
            data[4 * i + 1],
            data[4 * i + 2],
            data[4 * i + 3],
        ])
    };
    // decorations by target id: location, patch, builtin
    let mut deco: BTreeMap<u32, (Option<u32>, bool, Option<u32>)> = BTreeMap::new();
    let mut var: BTreeMap<u32, bool> = BTreeMap::new();
    let mut i = 5usize;
    while i < nw {
        let (n, op) = ((w(i) >> 16) as usize, w(i) & 0xffff);
        if n == 0 || i + n > nw {
            return Err("tessellation: invalid SPIR-V".into());
        }
        if op == 71 && n >= 3 {
            let d = deco.entry(w(i + 1)).or_default();
            let kind = w(i + 2);
            if kind == 30 && n >= 4 {
                d.0 = Some(w(i + 3));
            } else if kind == 15 {
                d.1 = true;
            } else if kind == 11 && n >= 4 {
                d.2 = Some(w(i + 3));
            }
        } else if op == 59 && n >= 4 {
            let sc = w(i + 3);
            if sc == 1 || sc == 3 {
                var.insert(w(i + 2), sc == 1);
            }
        }
        i += n;
    }
    Ok(var
        .into_iter()
        .map(|(v, input)| {
            let d = deco.get(&v).copied().unwrap_or((None, false, None));
            (
                v,
                Io {
                    input,
                    location: d.0,
                    patch: d.1,
                    builtin: d.2,
                },
            )
        })
        .collect())
}

re!(
    RE_MEMBER,
    r"^\s*(float|int|uint)(\d?) (\w+)(?: \[\[([^\]]*)\]\])?;\s*$"
);
re!(RE_M_NAME, r"^m_(\d+)$");

/// the parts of one `_MEMBER` match
struct Member<'a> {
    ty: &'a str,
    n: &'a str,
    name: &'a str,
    attr: Option<&'a str>,
}

fn member(ln: &str) -> Option<Member<'_>> {
    let c = RE_MEMBER.captures(ln)?;
    Some(Member {
        ty: c.get(1)?.as_str(),
        n: c.get(2)?.as_str(),
        name: c.get(3)?.as_str(),
        attr: c.get(4).map(|g| g.as_str()),
    })
}

/// Python truthiness of an optional group (`None` and `''` are both false)
fn truthy(a: Option<&str>) -> bool {
    a.map(|s| !s.is_empty()).unwrap_or(false)
}

/// `_struct(src, name)`: (start, end of the `struct name { ... };` match, the non-blank lines of its body)
fn struct_of<'a>(src: &'a str, name: &str) -> Option<(usize, usize, Vec<&'a str>)> {
    // one compiled pattern per struct name and thread (`rx::cached`: no lock shared by the build's workers)
    let re = super::rx::cached(&format!(
        r"(?s)struct {}\s*\{{(.*?)\}};",
        regex::escape(name)
    ))
    .expect("struct regex");
    let m = re.captures(src)?;
    let (all, body) = (m.get(0)?, m.get(1)?);
    Some((
        all.start(),
        all.end(),
        splitlines(body.as_str())
            .into_iter()
            .filter(|l| !strip(l).is_empty())
            .collect(),
    ))
}

/// `_location(io, member, storage, patch)`: a struct member's location from its SPIR-V variable (SPIRV-Cross names unnamed variables `m_<id>`)
fn location(io: &BTreeMap<u32, Io>, member: &str, input: bool, patch: bool) -> Result<u32, String> {
    let Some(c) = RE_M_NAME.captures(member) else {
        return Err(format!(
            "tessellation: member {member} is not a SPIR-V variable name"
        ));
    };
    let v = c[1].parse::<u32>().ok().and_then(|id| io.get(&id));
    let bad = || {
        format!(
            "tessellation: no {}{}put location for {member}",
            if patch { "patch " } else { "" },
            if input { "in" } else { "out" }
        )
    };
    let Some(v) = v else { return Err(bad()) };
    match v.location {
        Some(l) if v.input == input && v.patch == patch && l < 32 => Ok(l),
        _ => Err(bad()),
    }
}

/// `_slot_struct(name, members, slots, position_slot)`: a struct of float4-sized slots keeping the member names
fn slot_struct(
    name: &str,
    members: &[(String, String, String, usize)],
    slots: usize,
) -> Result<String, String> {
    let mut by: BTreeMap<usize, (String, String)> = BTreeMap::new();
    for (ty, n, mem, slot) in members {
        if n != "4" || by.contains_key(slot) {
            return Err(format!(
                "tessellation: {name}.{mem} is not one 4-component slot"
            ));
        }
        by.insert(*slot, (format!("{ty}4"), mem.clone()));
    }
    let mut lines = Vec::new();
    for s in 0..slots {
        let (ty, mem) = by
            .get(&s)
            .cloned()
            .unwrap_or_else(|| ("float4".to_string(), format!("dm_p{s}")));
        lines.push(format!("    {ty} {mem};"));
    }
    Ok(format!("struct {name}\n{{\n{}\n}};", lines.join("\n")))
}

/// `_words(ty, n, base)`: the capture record's words as the member's value
fn words(ty: &str, n: &str, base: usize) -> String {
    let k: usize = if n.is_empty() {
        1
    } else {
        n.parse().unwrap_or(1)
    };
    let ws = (0..k)
        .map(|c| format!("dm_r[{}]", base + c))
        .collect::<Vec<_>>()
        .join(", ");
    let vec = format!("uint{}", if k > 1 { k.to_string() } else { String::new() });
    let t = format!("{ty}{}", if k > 1 { k.to_string() } else { String::new() });
    if ty != "uint" {
        format!("as_type<{t}>({vec}({ws}))")
    } else {
        format!("{vec}({ws})")
    }
}

re!(
    RE_HULL_ENTRY,
    r"\bkernel void main0\(main0_in in \[\[stage_in\]\], "
);
re!(
    RE_HULL_OUTCP,
    r"device main0_out\* gl_out = &spvOut\[gl_PrimitiveID \* (\d+)\];"
);
re!(
    RE_HULL_PARAM,
    r"(threadgroup main0_in\* gl_in \[\[threadgroup\(0\)\]\])\)"
);
re!(RE_HULL_BODY, r"kernel void main0\([^{]*\)\s*\{\n");
re!(RE_GL_IN, r"\bgl_in\[(_\d+)\]");

/// what `hull` reports: `{'out_cp', 'in_bytes', 'cp_slots', 'patch_slots'}`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HullFacts {
    pub out_cp: i64,
    pub in_bytes: i64,
}

impl HullFacts {
    pub fn to_j(&self) -> J {
        J::obj()
            .with("out_cp", self.out_cp)
            .with("in_bytes", self.in_bytes)
            .with("cp_slots", CP_SLOTS)
            .with("patch_slots", PATCH_SLOTS)
    }
}

/// `hull(src, spv)`: rewrite a SPIRV-Cross tessellation-control kernel
pub fn hull(src: &str, spv: &[u8]) -> Result<(String, HullFacts), String> {
    let io = spirv_io(spv)?;
    if !RE_HULL_ENTRY.is_match(src) {
        return Err("tessellation: hull kernel without a stage_in control point".into());
    }
    let Some(out_cp) = RE_HULL_OUTCP.captures(src) else {
        return Err("tessellation: hull output control point count not found".into());
    };
    let out_cp: i64 = out_cp[1]
        .parse()
        .map_err(|_| "tessellation: hull output control point count not found".to_string())?;
    let Some((ms, me, lines)) = struct_of(src, "main0_in") else {
        return Err("tessellation: hull without main0_in".into());
    };
    let (mut loads, mut plain, mut in_bytes) = (Vec::<String>::new(), Vec::<String>::new(), 0i64);
    for ln in lines {
        let v = member(ln);
        let unknown = || format!("tessellation: unknown hull input {}", strip(ln));
        let Some(v) = v.filter(|v| truthy(v.attr) && v.attr.unwrap().starts_with("attribute("))
        else {
            return Err(unknown());
        };
        let base = if v.name == "gl_Position" {
            0
        } else {
            let loc = location(&io, v.name, true, false)?;
            let a = v.attr.unwrap();
            // int(group(4)[10:-1])
            let inner: String = {
                let cs: Vec<char> = a.chars().collect();
                if cs.len() > 11 {
                    cs[10..cs.len() - 1].iter().collect()
                } else {
                    String::new()
                }
            };
            let n = py_int(&inner)?;
            if n != loc as i128 {
                return Err(format!(
                    "tessellation: hull input {} attribute is not its location",
                    v.name
                ));
            }
            16 + 4 * loc as usize
        };
        loads.push(format!("in.{} = {};", v.name, words(v.ty, v.n, base)));
        plain.push(format!("    {}{} {};", v.ty, v.n, v.name));
        in_bytes += 16;
    }
    let mut src = format!(
        "{}struct main0_in\n{{\n{}\n}};\n\n// Maxwell reads an input control point past the patch (the translator's index 0x80000000 for one) as zero\nstatic inline main0_in dm_gin(threadgroup main0_in* p, int i, uint n) {{ return uint(i) < n ? p[i] : main0_in{{}}; }}{}",
        &src[..ms],
        plain.join("\n"),
        &src[me..]
    );
    for (name, slots, patch) in [
        ("main0_out", CP_SLOTS, false),
        ("main0_patchOut", PATCH_SLOTS, true),
    ] {
        let Some((s, e, lines)) = struct_of(&src, name) else {
            if name == "main0_out" {
                return Err("tessellation: hull without main0_out".into());
            }
            continue;
        };
        let mut members: Vec<(String, String, String, usize)> = Vec::new();
        for ln in lines {
            let Some(v) = member(ln).filter(|v| !truthy(v.attr)) else {
                return Err(format!("tessellation: unknown hull output {}", strip(ln)));
            };
            let n = if v.n.is_empty() { "1" } else { v.n };
            let slot = if v.name == "gl_Position" && !patch {
                CP_SLOTS - 1
            } else {
                location(&io, v.name, false, patch)? as usize
            };
            members.push((v.ty.to_string(), n.to_string(), v.name.to_string(), slot));
        }
        let text = slot_struct(name, &members, slots)?;
        src = format!("{}{}{}", &src[..s], text, &src[e..]);
    }
    src = src.replacen(
        "kernel void main0(main0_in in [[stage_in]], ",
        "kernel void main0(",
        1,
    );
    src = RE_HULL_PARAM.replacen(&src, 1, |c: &regex::Captures| format!("{}, device const uint* dm_tidx [[buffer({INDEX_BUFFER})]], device const uint* dm_tcap [[buffer({CAPTURE_BUFFER})]])", &c[1])).into_owned();
    if !src.contains("dm_tcap") {
        return Err("tessellation: hull kernel parameters not recognized".into());
    }
    let Some(body) = RE_HULL_BODY.find(&src) else {
        return Err("tessellation: hull kernel body not found".into());
    };
    let pro = format!(
        "    main0_in in = {{}};\n    if (gl_InvocationID < spvIndirectParams[0])\n    {{\n        device const uint* dm_r = dm_tcap + ulong(dm_tidx[gl_PrimitiveID * spvIndirectParams[0] + gl_InvocationID]) * {CAPTURE_WORDS}u;\n{}    }}\n",
        loads
            .iter()
            .map(|x| format!("        {x}\n"))
            .collect::<String>()
    );
    src = format!("{}{}{}", &src[..body.end()], pro, &src[body.end()..]);
    src = RE_GL_IN
        .replace_all(&src, "dm_gin(gl_in, $1, spvIndirectParams[0])")
        .into_owned();
    if !src.contains("gl_in[gl_InvocationID] = in;") {
        return Err("tessellation: hull without its control point store".into());
    }
    Ok((src, HullFacts { out_cp, in_bytes }))
}

re!(
    RE_DOM_ENTRY,
    r"(?s)\[\[ patch\(\w+, \d+\) \]\] vertex main0_out main0\((.*?)\)\s*\{\n"
);
re!(RE_DOM_PRIM, r"^uint gl_PrimitiveID \[\[patch_id\]\]$");
re!(RE_OUT_STRUCT, r"(?s)struct main0_out\s*\{(.*?)\};");
re!(
    RE_POSITION,
    r"float4 (\w+) \[\[position(?:, invariant)?\]\];"
);

/// what `domain` reports
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainFacts {
    pub mode: String,
    pub spacing: String,
    pub winding: String,
    pub md: u32,
}

impl DomainFacts {
    pub fn to_j(&self) -> J {
        J::obj()
            .with("mode", self.mode.as_str())
            .with("spacing", self.spacing.as_str())
            .with("winding", self.winding.as_str())
            .with("md", self.md)
            .with("cp_slots", CP_SLOTS)
            .with("patch_slots", PATCH_SLOTS)
    }
}

/// `domain(src, spv, mode)`: rewrite a SPIRV-Cross post-tessellation vertex function into a plain vertex function. `mode_repr` is what the Python's message prints for a missing mode (`None`)
pub fn domain(src: &str, spv: &[u8], mode: Option<&Mode>) -> Result<(String, DomainFacts), String> {
    let mode = match mode {
        Some(m) if m.mode == "quads" || m.mode == "triangles" => m,
        Some(m) => {
            return Err(format!(
                "tessellation: domain mode {} is not implemented (quads and triangles)",
                m.mode
            ));
        }
        None => {
            return Err(
                "tessellation: domain mode None is not implemented (quads and triangles)".into(),
            );
        }
    };
    let md = mode_bits(mode)?;
    let io = spirv_io(spv)?;
    let Some(entry) = RE_DOM_ENTRY.captures(src) else {
        return Err("tessellation: domain without a patch vertex entry".into());
    };
    let params: Vec<&str> = entry
        .get(1)
        .unwrap()
        .as_str()
        .split(", ")
        .map(strip)
        .collect();
    let mut keep: Vec<String> = Vec::new();
    for p in &params {
        if *p == "main0_patchIn patchIn [[stage_in]]"
            || *p == "float3 gl_TessCoord [[position_in_patch]]"
        {
            continue;
        }
        if RE_DOM_PRIM.is_match(p) {
            continue;
        }
        if !p.starts_with("constant spvDescriptorSetBuffer0& ") {
            return Err(format!("tessellation: unknown domain parameter {p}"));
        }
        keep.push(p.to_string());
    }
    keep.push("uint dm_vid [[vertex_id]]".into());
    keep.push(format!(
        "device const float4* dm_tbuf [[buffer({CP_BUFFER})]]"
    ));
    keep.push(format!(
        "constant uint4* dm_tinfo [[buffer({INFO_BUFFER})]]"
    ));
    let mut src = src.to_string();
    let mut cp_loads: Vec<(String, String, usize)> = Vec::new();
    if let Some((ms, me, lines)) = struct_of(&src, "main0_in") {
        let mut plain: Vec<String> = Vec::new();
        for ln in lines {
            let Some(v) =
                member(ln).filter(|v| truthy(v.attr) && v.attr.unwrap().starts_with("attribute("))
            else {
                return Err(format!("tessellation: unknown domain input {}", strip(ln)));
            };
            let n = if v.n.is_empty() { "1" } else { v.n };
            if n != "4" {
                return Err(format!(
                    "tessellation: domain input {} is not a 4-component slot",
                    v.name
                ));
            }
            let slot = if v.name == "gl_Position" {
                CP_SLOTS - 1
            } else {
                location(&io, v.name, true, false)? as usize
            };
            cp_loads.push((v.name.to_string(), v.ty.to_string(), slot));
            plain.push(format!("    {}4 {};", v.ty, v.name));
        }
        src = format!(
            "{}struct main0_in\n{{\n{}\n}};{}",
            &src[..ms],
            plain.join("\n"),
            &src[me..]
        );
    }
    let Some((ms, me, lines)) = struct_of(&src, "main0_patchIn") else {
        return Err("tessellation: domain without main0_patchIn".into());
    };
    let (mut patch_loads, mut plain): (Vec<(String, String, u32)>, Vec<String>) =
        (Vec::new(), Vec::new());
    for ln in lines {
        if strip(ln) == "patch_control_point<main0_in> gl_in;" {
            plain.push("    dm_cps<main0_in> gl_in;".into());
            continue;
        }
        let Some(v) =
            member(ln).filter(|v| truthy(v.attr) && v.attr.unwrap().starts_with("attribute("))
        else {
            return Err(format!(
                "tessellation: unknown domain patch input {}",
                strip(ln)
            ));
        };
        let n = if v.n.is_empty() { "1" } else { v.n };
        if n != "4" {
            return Err(format!(
                "tessellation: domain patch input {} is not a 4-component slot",
                v.name
            ));
        }
        patch_loads.push((
            v.name.to_string(),
            v.ty.to_string(),
            location(&io, v.name, true, true)?,
        ));
        plain.push(format!("    {}4 {};", v.ty, v.name));
    }
    let helper = format!(
        "template<typename T> struct dm_cps\n{{\n    T e[32];\n    uint n;\n    T operator[](uint i) const thread {{ return i < n && i < 32u ? e[i] : T{{}}; }}\n    uint size() const thread {{ return n; }}\n}};\n\n{TESS_MSL}\n"
    );
    src = format!(
        "{}{}struct main0_patchIn\n{{\n{}\n}};{}",
        &src[..ms],
        helper,
        plain.join("\n"),
        &src[me..]
    );
    let entry = RE_DOM_ENTRY.captures(&src);
    let pos = RE_OUT_STRUCT.captures(&src).and_then(|o| {
        RE_POSITION
            .captures(o.get(1).unwrap().as_str())
            .map(|p| p[1].to_string())
    });
    if pos.is_none() {
        return Err("tessellation: domain output without a position".into());
    }
    let Some(entry) = entry else {
        return Err("tessellation: domain entry point not found".into());
    };
    let (e0, e1) = (entry.get(0).unwrap().start(), entry.get(0).unwrap().end());
    let ld = |ty: &str, at: String| {
        if ty != "float" {
            format!("as_type<{ty}4>({at})")
        } else {
            at
        }
    };
    let mut pro: Vec<String> = vec![
        "    uint dm_ncp = dm_tinfo[0].y;".into(),
        "    uint dm_e = ((device const uint*)((device const uchar*)dm_tbuf + dm_tinfo[1].y))[dm_vid / 6u];".into(),
        "    uint gl_PrimitiveID = dm_e >> 12, dm_k = dm_vid % 6u;".into(),
        "    device const half* dm_lev = (device const half*)((device const uchar*)dm_tbuf + dm_tinfo[1].x) + ulong(gl_PrimitiveID) * 6u;".into(),
        format!("    float3 gl_TessCoord = dm_tvert(dm_e & 4095u, dm_k, dm_lev, float(dm_tinfo[1].w), {md}u);"),
        format!("    device const float4* dm_cp = dm_tbuf + ulong(gl_PrimitiveID) * dm_ncp * {CP_SLOTS}u;"),
        format!("    device const float4* dm_pp = dm_tbuf + dm_tinfo[0].z / 16u + ulong(gl_PrimitiveID) * {PATCH_SLOTS}u;"),
        "    main0_patchIn patchIn;".into(),
        "    patchIn.gl_in.n = dm_ncp;".into(),
        "    for (uint dm_i = 0; dm_i < dm_ncp && dm_i < 32u; dm_i++)".into(),
        "    {".into(),
    ];
    for (mem, ty, slot) in &cp_loads {
        pro.push(format!(
            "        patchIn.gl_in.e[dm_i].{mem} = {};",
            ld(ty, format!("dm_cp[dm_i * {CP_SLOTS}u + {slot}u]"))
        ));
    }
    pro.push("    }".into());
    for (mem, ty, slot) in &patch_loads {
        pro.push(format!(
            "    patchIn.{mem} = {};",
            ld(ty, format!("dm_pp[{slot}u]"))
        ));
    }
    let body_start = e1;
    let tail = body_end(&src, body_start);
    let body = tail.map_or("", |t| &src[body_start..t]);
    let Some(tail) = tail.filter(|_| body.contains("return out;")) else {
        return Err("tessellation: domain without return".into());
    };
    let head = format!("vertex main0_out main0({})\n{{\n", keep.join(", "));
    let out = format!(
        "{}{}{}\n{}{}",
        &src[..e0],
        head,
        pro.join("\n"),
        body,
        &src[tail..]
    );
    Ok((
        out,
        DomainFacts {
            mode: mode.mode.clone(),
            spacing: mode.spacing.clone(),
            winding: mode.winding.clone(),
            md,
        },
    ))
}

/// Where the entry point's body ends (the byte of its closing brace) after the header that ends at `body_start`: the Python's `end - 1` after its brace-counting loop, in
/// characters. Without a closing brace the loop stops at the end of the text and `end - 1` is the last character (not the last byte: it may be a multi-byte one). `None`
/// when that is before `body_start` (the header ends the text): the Python's `src[body_start:end - 1]` is then empty.
fn body_end(src: &str, body_start: usize) -> Option<usize> {
    let b = src.as_bytes();
    let (mut end, mut depth) = (body_start, 1i32);
    while depth != 0 && end < b.len() {
        depth += (b[end] == b'{') as i32 - (b[end] == b'}') as i32;
        end += 1;
    }
    let last = if depth == 0 {
        end - 1
    } else {
        src[..end].char_indices().next_back()?.0
    };
    (last >= body_start).then_some(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_body_ends_at_its_brace_or_at_the_last_character() {
        let s = "head{\n x { y } return out;\n}\nrest";
        assert_eq!(body_end(s, 5), Some(s.rfind('}').unwrap()));
        // the header ends the text: no body (this sliced src[len..len - 1] and panicked)
        assert_eq!(body_end("head{\n", 6), None);
        // no closing brace: up to the last character, a multi-byte one kept whole (a byte cut panicked)
        let s = "head{\n return out; é";
        assert_eq!(body_end(s, 6), Some(s.len() - 'é'.len_utf8()));
    }

    #[test]
    fn control_mode_and_bits() {
        let mut d = vec![0u8; 0x748];
        d[0x738..0x73c].copy_from_slice(&7u32.to_le_bytes());
        d[0x73c..0x740].copy_from_slice(&0x8E7Cu32.to_le_bytes());
        d[0x744..0x748].copy_from_slice(&0x0900u32.to_le_bytes());
        let m = control_mode(&d).unwrap();
        assert_eq!(
            (m.mode.as_str(), m.spacing.as_str(), m.winding.as_str()),
            ("quads", "fractional_even", "cw")
        );
        assert_eq!(mode_bits(&m).unwrap(), 2 << 1 | 8);
        assert!(control_mode(&d[..0x747]).is_none());
    }

    #[test]
    fn spirv_io_rejects_a_broken_module() {
        let mut w = vec![0u8; 20];
        w.extend_from_slice(&((9u32 << 16) | 59).to_le_bytes());
        assert_eq!(spirv_io(&w).unwrap_err(), "tessellation: invalid SPIR-V");
        assert!(spirv_io(&[0u8; 20]).unwrap().is_empty());
    }
}
