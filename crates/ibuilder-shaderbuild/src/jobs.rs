//! `dmsh_build.job_for` and its helpers (`vertex_ab`, `early_fragment_tests`, `ps_omap`, `tf_layout`) plus the two `dmsh_ampere` / `dmsh_tess` readers they call:
//! a split record -> the translator's job (a JSON object), the code bytes it reads (`<key>.bin`) and the `c1` constant segment.
//!
//! Byte-identity with the Python is the point: the job's keys are in the Python dict's insertion order (`key, stage, code, tex, img, attr, fout, c1, <extra>,
//! debug, reduced`, then the answer table's keys: existing ones keep their place, new ones follow), and `pyjson` writes them the way `json.dump` does.
//!
//! The Switch 2 task / mesh stages (`dmsh_meshstage.py`: `info`, `memory`, `own_shared`) are ported (round 3); [`JobError::Unported`] stays for what a later round may meet.
use serde_json::{Map, Value, json};

/// A shader the build cannot give a job: the Python's `sys.exit('[RED] ...')` or `job_for`'s own `ValueError` (a ray tracing record on the ordinary path; its text has no `[RED]`):
/// a failed shader here, a stopped build there; or a case this port does not cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    Red(String),
    Unported(String),
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::Red(m) => f.write_str(m),
            JobError::Unported(m) => write!(f, "unported: {m}"),
        }
    }
}

/// The translation options of the job (`XLATE_DEBUG` / `XLATE_REDUCED` of the environment: `debug` unless `XLATE_DEBUG=0`, `reduced` only when `XLATE_REDUCED=1`; both default on)
#[derive(Clone, Copy, Debug)]
pub struct XlateOpts {
    pub debug: bool,
    pub reduced: bool,
}

impl Default for XlateOpts {
    /// `debug` is on unless the app's "Optimized translator (experimental)" setting is (`set_optimized`, speed-up B9): the translator's DebugMode skips its AST optimizer, so its output
    /// is full of temporaries (dmsh_xlate Program.cs: "semantics unchanged"); without it the MSL is smaller and every later step is faster, but outputs change: a library built with it
    /// differs from one built without it (the option is in every shader's digest, `key.rs`).
    fn default() -> Self {
        XlateOpts {
            debug: !optimized(),
            reduced: true,
        }
    }
}

/// The app's "Optimized translator (experimental)" setting (`ibuilder_pipeline::sys::qos::set_xlate_optimized`, set by the FFI at every `ibuilder_run_chain`)
pub fn set_optimized(on: bool) {
    ibuilder_pipeline::sys::qos::set_xlate_optimized(on);
}

/// `true` while the translator runs without its DebugMode (`set_optimized`).
pub fn optimized() -> bool {
    ibuilder_pipeline::sys::qos::xlate_optimized()
}

#[derive(Debug, Clone)]
pub struct JobOut {
    /// the job object as `job_for` returns it (`code` is the path given)
    pub job: Map<String, Value>,
    /// what `<key>.bin` holds
    pub code: Vec<u8>,
    pub c1: Vec<u8>,
    /// the `[RED]` lines the Python prints (`tf_layout`): the shader goes on
    pub notes: Vec<String>,
}

pub const CTL_MAGIC: [u8; 4] = [0x34, 0x12, 0x76, 0x98];
const GFX_MAGIC: [u8; 4] = [0x78, 0x56, 0x34, 0x12];
const CS_MAGIC: [u8; 4] = [0x79, 0x56, 0x34, 0x12];
const NVN2_VERSION: u32 = 0xc9;
/// `dmsh_abi.NVN2_STAGE_BANK` (`DMSH_NVN2_STAGE_BANK` in `dmsh_abi.h`)
const NVN2_STAGE_BANK: i64 = 25;

fn short(what: &str) -> JobError {
    JobError::Red(format!(
        "{what}: the record is too short for the field the translator's job reads"
    ))
}

fn rd32(b: &[u8], off: usize, what: &str) -> Result<u32, JobError> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| short(what))
}
fn rd16(b: &[u8], off: usize, what: &str) -> Result<u16, JobError> {
    b.get(off..off + 2)
        .map(|s| u16::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| short(what))
}
fn rdb(b: &[u8], off: usize, what: &str) -> Result<u8, JobError> {
    b.get(off).copied().ok_or_else(|| short(what))
}

/// `bytes[a:b]` with Python's clamping
fn slice(b: &[u8], a: usize, e: usize) -> &[u8] {
    let e = e.min(b.len());
    if a >= e { &[] } else { &b[a..e] }
}

pub(crate) fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > hay.len() {
        return None;
    }
    memchr::memmem::find(&hay[from..], needle).map(|i| i + from)
}

/// `dmsh_ampere.is_nvn2`: magic and version 0xc9
pub fn is_nvn2(ctl: &[u8]) -> bool {
    ctl.len() >= 0x48
        && u32::from_le_bytes(ctl[..4].try_into().unwrap()) == 0x98761234
        && u32::from_le_bytes(ctl[4..8].try_into().unwrap()) == NVN2_VERSION
}

/// `dmsh_ampere.early_fragment_tests`: control byte +0x5b
fn nvn2_early_fragment_tests(ctl: &[u8]) -> bool {
    ctl.len() > 0x5b && ctl[0x5b] != 0
}

/// `dmsh_build.early_fragment_tests`: a pixel shader's early fragment tests flag unless its SPH writes depth (word 19 bit 1)
pub fn early_fragment_tests(ctl: &[u8], code: &[u8]) -> bool {
    let flag = if is_nvn2(ctl) {
        nvn2_early_fragment_tests(ctl)
    } else {
        ctl.len() > 0x718 && ctl[0x718] != 0
    };
    flag && code.len() >= 0x50
        && u32::from_le_bytes(code[19 * 4..19 * 4 + 4].try_into().unwrap()) & 2 == 0
}

/// `dmsh_build.ps_omap`: SPH word 18, the output map (None for a code area shorter than the SPH)
pub fn ps_omap(code: &[u8]) -> Option<u32> {
    if code.len() >= 0x50 {
        Some(u32::from_le_bytes(
            code[18 * 4..18 * 4 + 4].try_into().unwrap(),
        ))
    } else {
        None
    }
}

/// The two byte vectors of a combined vertex shader: (code, c1).
pub type VertexAb = (Vec<u8>, Vec<u8>);

/// `dmsh_build.vertex_ab`: Maxwell's type-0 VertexA and type-1 VertexB merged into one program as Ryujinx does. `Ok(Some((code, c1)))` for such a shader (c1 follows part B when
/// control +0x700 places B's header, else it lies between +0x700's offset and B), `Ok(None)` for a single program.
pub fn vertex_ab(key: &str, code: &[u8], ctl: &[u8]) -> Result<Option<VertexAb>, JobError> {
    if code.len() < 0x80 || ctl.len() < 0x760 || code[..4] != GFX_MAGIC {
        return Ok(None);
    }
    let w0 = rd32(code, 0x30, "vertex_ab")?;
    if w0 & 0x1f != 1 || (w0 >> 10) & 0xf != 0 {
        return Ok(None);
    }
    let h: Vec<u32> = (0..6)
        .map(|i| rd32(ctl, 0x748 + 4 * i, "vertex_ab"))
        .collect::<Result<_, _>>()?;
    let (a_sph, a_len, b_sph, b_len) = (h[0] as usize, h[1] as usize, h[3] as usize, h[4] as usize);
    if a_sph == 0 && a_len == 0 && b_sph == 0 && b_len == 0 {
        return Ok(None); // one program of shader type 0, no part B: translated as it is
    }
    let bw0 = if 0x80 <= b_sph && b_sph <= code.len() - 0x50 {
        rd32(code, b_sph, "vertex_ab")?
    } else {
        0
    };
    let c1o = rd32(ctl, 0x700, "vertex_ab")? as usize;
    let (a_end, b_end) = (a_sph + a_len, b_sph + b_len);
    if a_sph != 0x30
        || !(a_end <= c1o && c1o <= b_sph)
        || b_end > code.len()
        || bw0 & 0x1f != 1
        || (bw0 >> 10) & 0xf != 1
        || a_len < 0x50
        || b_len < 0x50
        || !(a_len - 0x50).is_multiple_of(32)
        || !(b_len - 0x50).is_multiple_of(32)
    {
        return Err(JobError::Red(format!(
            "[RED] {key}: vertex shader of shader type 0 without a VertexB part in the expected place (control +0x748 {a_sph:#x} {a_len:#x} {b_sph:#x} {b_len:#x}, +0x700 {c1o:#x})"
        )));
    }
    // Catherine puts B's 0x30-byte header at +0x700's offset and c1 after B; Borderlands uses the offset for c1 before B
    let trailing = &code[b_end..];
    let c1 = if c1o + 0x30 == b_sph {
        trailing.to_vec()
    } else {
        if trailing.iter().any(|b| *b != 0) {
            return Err(JobError::Red(format!(
                "[RED] {key}: two-part vertex shader whose part B carries constants of its own: a merged program has one c1"
            )));
        }
        code[c1o..b_sph].to_vec()
    };
    let word = |off: usize, i: usize| {
        u32::from_le_bytes(code[off + 4 * i..off + 4 * i + 4].try_into().unwrap())
    };
    let sa: Vec<u32> = (0..20).map(|i| word(a_sph, i)).collect();
    let sb: Vec<u32> = (0..20).map(|i| word(b_sph, i)).collect();
    let mut sph: Vec<u32> = Vec::with_capacity(20);
    sph.push(sb[0] | (sa[0] & !(0xfu32 << 10)));
    for i in 1..4 {
        sph.push((sa[i] & 0xffffff).max(sb[i] & 0xffffff) | (sb[i] & !0xffffffu32));
    }
    sph.push(sb[4]);
    for i in 5..20 {
        sph.push(sa[i] | sb[i]);
    }
    let mut a = code[a_sph + 0x50..a_end].to_vec();
    let b = &code[b_sph + 0x50..b_end];
    let alen = a.len();
    let mut o = 0usize;
    while o < alen {
        if !o.is_multiple_of(32) && o + 8 <= alen {
            let ins = u64::from_le_bytes(a[o..o + 8].try_into().unwrap());
            if ins >> 52 == 0xe30 && ins & 0x1f == 0xf {
                // EXIT with CC T (any predicate) -> BRA to part B, keeping the predicate and CC bits; the offset counts from the next instruction
                let new =
                    0xe24u64 << 52 | (((alen - (o + 8)) as u64 & 0xffffff) << 20) | (ins & 0xfffff);
                a[o..o + 8].copy_from_slice(&new.to_le_bytes());
            }
        }
        o += 8;
    }
    let mut out = Vec::with_capacity(0x30 + 80 + a.len() + b.len());
    out.extend_from_slice(&code[..0x30]);
    for w in &sph {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(&a);
    out.extend_from_slice(b);
    Ok(Some((out, c1)))
}

/// `dmsh_build.tf_layout`: the four transform feedback streams of a vertex / geometry shader's control section (None: no strides, or a layout of unknown form: `notes` gets the Python's `[RED]` line)
pub fn tf_layout(
    key: &str,
    ctl: &[u8],
    notes: &mut Vec<String>,
) -> Result<Option<Value>, JobError> {
    let strides: Vec<u16> = (0..4)
        .map(|i| rd16(ctl, 0x6dc + 2 * i, "tf_layout"))
        .collect::<Result<_, _>>()?;
    if strides.iter().all(|s| *s == 0) {
        return Ok(None);
    }
    let mut out: Vec<Value> = Vec::new();
    #[allow(clippy::needless_range_loop)] // `b` also indexes `ctl` and offsets the per-buffer reads
    for b in 0..4usize {
        if strides[b] == 0 {
            out.push(json!([]));
            continue;
        }
        let count = rdb(ctl, 0x6e8 + b, "tf_layout")? as usize;
        let locs: Vec<u8> = slice(ctl, 0xdc + 0x80 * b, 0xdc + 0x80 * b + count).to_vec();
        let sizes: Vec<u8> = slice(ctl, 0x4dc + 0x80 * b, 0x4dc + 0x80 * b + count).to_vec();
        let bad = !(1..=128).contains(&count)
            || locs.iter().any(|x| !(0x20..0x40).contains(x))
            || sizes.iter().any(|n| !(1..=4).contains(n))
            || strides[b] as usize != 4 * sizes.iter().map(|n| *n as usize).sum::<usize>();
        if bad {
            // the Python prints the lists with Python's repr
            let l: Vec<String> = locs.iter().map(|x| x.to_string()).collect();
            let s: Vec<String> = sizes.iter().map(|x| x.to_string()).collect();
            notes.push(format!(
                "[RED] {key}: transform feedback buffer {b} has a layout of unknown form (stride {}, outputs [{}], components [{}]): no capture entry point; see tf_layout",
                strides[b],
                l.join(", "),
                s.join(", ")
            ));
            return Ok(None);
        }
        let outputs: Vec<Value> = locs
            .iter()
            .zip(&sizes)
            .map(|(o, n)| json!([*o as i64 - 0x20, *n as i64]))
            .collect();
        out.push(json!({"stride": strides[b], "outputs": outputs}));
    }
    Ok(Some(Value::Array(out)))
}

/// `dmsh_tess.control_mode`: a domain shader's tessellation mode from its control section (None: not a domain control section)
fn tess_control_mode(ctl: &[u8]) -> Option<Value> {
    if ctl.len() < 0x748 {
        return None;
    }
    let u = |o: usize| u32::from_le_bytes(ctl[o..o + 4].try_into().unwrap());
    let (mode, spacing, order) = (u(0x738), u(0x73c), u(0x744));
    let m = match mode {
        0x0004 => "triangles",
        0x0007 => "quads",
        0x8E7A => "isolines",
        _ => return None,
    };
    let s = match spacing {
        0x0202 => "equal",
        0x8E7B => "fractional_odd",
        0x8E7C => "fractional_even",
        _ => return None,
    };
    let w = match order {
        0x0900 => "cw",
        0x0901 => "ccw",
        _ => return None,
    };
    Some(json!({"mode": m, "spacing": s, "winding": w}))
}

fn nvn2_stage_index(stage: &str) -> Option<i64> {
    match stage {
        "vs" => Some(0),
        "hs" | "ms" => Some(1),
        "ds" => Some(2),
        "gs" => Some(3),
        "ps" => Some(4),
        _ => None,
    }
}

/// `dmsh_build.job_for(key, name, rec, w)` without the file write: `name` is the package file name (`ps_main.bin`: its first two letters are the stage), `rec` the whole record,
/// `code_path` what the job's `code` field says (`<work>/<key>.bin`), `facts` the answer-table object of the key (None: `no_facts`).
pub fn job_for(
    key: &str,
    name: &str,
    rec: &[u8],
    code_path: &str,
    facts: Option<&Value>,
    opts: &XlateOpts,
) -> Result<JobOut, JobError> {
    let mut notes = Vec::new();
    let ci = find(rec, &CTL_MAGIC, 0x38).ok_or_else(|| {
        JobError::Red(format!("[RED] {key}: the record holds no control section"))
    })?;
    let mut code: Vec<u8> = slice(rec, 0x38, ci).to_vec();
    let mut stage: String = name.chars().take(2).collect();
    let ctl = &rec[ci..];
    // a ray tracing record (NVN2 stage byte 8) belongs to the RT build: the Python's `ValueError` (the build stops), its text as it is (no `[RED]`). Short NVN2 graphics control sections have no stage byte
    if is_nvn2(ctl) && ctl.len() > 0x56 && ctl[0x56] == 8 {
        return Err(JobError::Red(format!(
            "{key}: stage8 RT record reached the ordinary shader path; correct its shader_map stage"
        )));
    }
    let mut extra: Vec<(String, Value)> = Vec::new();
    let mut mesh_job = false;
    let c1: Vec<u8>;
    if is_nvn2(ctl) {
        // NVN2 (Switch 2): Ampere code, a 0x80-byte SPH for the graphics stages, then the instructions; c1 lies between control +0x40 and +0x44
        let a = rd32(ctl, 0x40, "nvn2 c1")? as usize;
        let e = rd32(ctl, 0x44, "nvn2 c1")? as usize;
        c1 = if a <= e && e <= code.len() {
            code[a..e].to_vec()
        } else {
            Vec::new()
        };
        let inst = rd32(ctl, 0x38, "nvn2 inst_len")? as usize;
        code.truncate(inst);
        extra.push(("arch".into(), json!("sm86")));
        if stage == "cs" {
            let v = json!([
                rdb(ctl, 0xb1, "nvn2_cs")?,
                rdb(ctl, 0xb2, "nvn2_cs")?,
                rdb(ctl, 0xb3, "nvn2_cs")?,
                rd16(ctl, 0xba, "nvn2_cs")?,
                rdb(ctl, 0xa9, "nvn2_cs")? as u32 * 4,
                rdb(ctl, 0xb4, "nvn2_cs")?,
                rdb(ctl, 0xb5, "nvn2_cs")?,
                rdb(ctl, 0xaa, "nvn2_cs")? as u32 * 4
            ]);
            extra.push(("nvn2_cs".into(), v));
        }
        if let Some(i) = nvn2_stage_index(&stage) {
            extra.push(("stage_bank".into(), json!(NVN2_STAGE_BANK + i)));
        }
        if stage == "ts" || stage == "ms" {
            // A task or mesh shader: translated as compute, its SPH taken off and its stage memory in shared memory, then wrapped as the Metal stage function (dmsh_meshstage.py).
            // Graphics stages' c[0] and c[2] tables stay: the translator reads their addresses out of the code as for a vertex shader
            let red = |m: String| JobError::Red(format!("[RED] {key}: {m}"));
            let area = slice(rec, 0x38, ci);
            let mut mesh = ibuilder_shader::finish::meshstage::info(ctl, area).map_err(&red)?;
            if mesh.kind.is_empty() {
                return Err(red("KeyError: 'max_v' (the control section's stage byte is neither a task nor a mesh shader's)".into()));
            }
            let (lay, size) = ibuilder_shader::finish::meshstage::memory(&mesh);
            let inst_code = slice(&code, 0x80, usize::MAX);
            if ibuilder_shader::finish::meshstage::own_shared(inst_code)
                || size > ibuilder_shader::finish::meshstage::METAL_MAX_SHARED
            {
                mesh.refused =
                    Some("shared memory of its own, or stage memory past Metal's limit".into());
            }
            // a task-fed mesh shader whose payload goes through the spill buffer reads it from a shared memory of its own (the translator's `dm_stage_in`)
            if mesh.kind == "ms"
                && mesh.task == Some(true)
                && ibuilder_shader::finish::meshstage::payload_spill(&mesh)
            {
                extra.push((
                    "stage_input_words".into(),
                    json!(ibuilder_shader::finish::meshstage::payload_words(&mesh)),
                ));
            }
            extra.push(("mesh_stage".into(), mesh.to_json()));
            extra.push(("stage_memory".into(), json!(lay)));
            extra.push(("local".into(), json!([mesh.threads, 1, 1])));
            extra.push(("shared".into(), json!(size)));
            code = inst_code.to_vec();
            stage = "cs".into();
            mesh_job = true;
        }
    } else {
        // control segment +0x700: c1 relative to the start of the instruction area; +0x704: total length of the instruction area
        let c1o = rd32(ctl, 0x700, "c1 offset")? as usize;
        let c1t = rd32(ctl, 0x704, "c1 end")? as usize;
        let mut c1v = slice(rec, 0x38 + c1o, 0x38 + c1t).to_vec();
        if stage == "vs"
            && let Some((c, c1b)) = vertex_ab(key, &code, ctl)?
        {
            code = c;
            c1v = c1b;
        }
        if stage != "cs" {
            code = slice(&code, 0x30, usize::MAX).to_vec(); // strip the 0x30 header (the 0x50 SPH after it is read by the translator itself)
        } else if code.len() >= 4 && code[..4] == CS_MAGIC {
            // a compute header: 0x30 header + zero padding, the instructions start at code area +0x100
            if slice(&code, 0x30, 0x100).iter().any(|b| *b != 0) {
                return Err(JobError::Red(format!(
                    "[RED] {key}: compute code area +0x30..+0x100 is not all zero, so the instructions do not start at +0x100"
                )));
            }
            code = slice(&code, 0x100, usize::MAX).to_vec();
        }
        if (stage == "vs" || stage == "gs")
            && let Some(tf) = tf_layout(key, ctl, &mut notes)?
        {
            extra.push(("tf".into(), tf));
        }
        c1 = c1v;
    }
    if stage == "ps" && early_fragment_tests(ctl, &code) {
        extra.push(("early_fragment_tests".into(), json!(true)));
    }
    if stage == "ds"
        && let Some(tm) = tess_control_mode(ctl)
    {
        extra.push(("tess_mode".into(), tm));
    }
    let mut j = Map::new();
    j.insert("key".into(), json!(key));
    j.insert("stage".into(), json!(stage));
    j.insert("code".into(), json!(code_path));
    j.insert("tex".into(), json!({}));
    j.insert("img".into(), json!({}));
    j.insert("attr".into(), json!({}));
    j.insert("fout".into(), json!({}));
    j.insert("c1".into(), json!(crate::records::hex(&c1)));
    for (k, v) in extra {
        j.insert(k, v);
    }
    j.insert("debug".into(), json!(opts.debug));
    j.insert("reduced".into(), json!(opts.reduced));
    if mesh_job {
        return Ok(JobOut {
            job: j,
            code,
            c1,
            notes,
        }); // its threads and memory come from its control section (above): no answer-table row
    }
    match facts {
        None => {
            j.insert("no_facts".into(), json!(true));
        }
        Some(Value::Object(f)) => {
            for (k, v) in f {
                j.insert(k.clone(), v.clone());
            }
        }
        Some(_) => {
            return Err(JobError::Red(format!(
                "[RED] {key}: its answer-table row is not an object"
            )));
        }
    }
    Ok(JobOut {
        job: j,
        code,
        c1,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_optimized_translator_setting_turns_debug_off() {
        assert!(XlateOpts::default().debug);
        set_optimized(true);
        assert!(!XlateOpts::default().debug && XlateOpts::default().reduced);
        set_optimized(false);
        assert!(XlateOpts::default().debug);
    }

    #[test]
    fn tf_and_domain_readers() {
        let mut ctl = vec![0u8; 0x760];
        let mut notes = Vec::new();
        assert_eq!(tf_layout("k", &ctl, &mut notes).unwrap(), None);
        ctl[0x6dc] = 16; // stream 0: stride 16, one output at location 0x20 with 4 components
        ctl[0x6e8] = 1;
        ctl[0xdc] = 0x21;
        ctl[0x4dc] = 4;
        assert_eq!(
            tf_layout("k", &ctl, &mut notes).unwrap(),
            Some(json!([{"stride": 16, "outputs": [[1, 4]]}, [], [], []]))
        );
        ctl[0x4dc] = 3; // stride no longer 4 * sum(sizes)
        assert_eq!(tf_layout("k", &ctl, &mut notes).unwrap(), None);
        assert!(notes[0].starts_with("[RED] k: transform feedback buffer 0 has a layout of unknown form (stride 16, outputs [33], components [3])"));
        let mut d = vec![0u8; 0x760];
        d[0x738..0x73c].copy_from_slice(&4u32.to_le_bytes());
        d[0x73c..0x740].copy_from_slice(&0x0202u32.to_le_bytes());
        d[0x744..0x748].copy_from_slice(&0x0900u32.to_le_bytes());
        assert_eq!(
            tess_control_mode(&d),
            Some(json!({"mode": "triangles", "spacing": "equal", "winding": "cw"}))
        );
    }
}
