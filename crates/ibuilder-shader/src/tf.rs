//! Transform feedback: dmsh_build.py `vs_tf` (the capture entry point of a vertex shader) and `tf_layout` (the four feedback streams of a program's control section).
use crate::text::*;
use crate::{PostError, failed};
use serde_json::Value;

/// Metal vertex buffer index of transform feedback buffer 0 (`TF_SLOT`)
pub const TF_SLOT: u32 = 8;

/// Python truthiness of a JSON value (`if not e`)
fn falsy(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// `\bvertex\s+main0_out\s+main0\s*\((.*?)\)\s*\{` (re.S) -> (params range, end of the match)
fn main_header(src: &str) -> Option<(usize, usize, usize)> {
    let (_, pe) = crate::vs::main_entry_m(src.as_bytes(), "main0", 0)?;
    let b = src.as_bytes();
    // `(.*?)\)\s*\{`: the first `)` followed by optional whitespace and `{`
    let mut from = pe;
    loop {
        let r = memchr::memchr(b')', &b[from..])? + from;
        let q = skip_space(b, r + 1);
        if q < b.len() && b[q] == b'{' {
            return Some((pe, r, q + 1));
        }
        from = r + 1;
    }
}

/// `uint\s+(\w+)\s*\[\[<sem>\]\]` in `params`: the first match's name
fn param_named<'a>(params: &'a str, sem: &str) -> Option<&'a str> {
    let b = params.as_bytes();
    let mut from = 0;
    while let Some(p) = find_b(b, b"uint", from) {
        from = p + 1;
        let mut c = Cur::at(params, p + 4);
        if !c.ws1() {
            continue;
        }
        let Some(name) = c.word() else { continue };
        c.ws0();
        if c.lit("[[") && c.lit(sem) && c.lit("]]") {
            return Some(name);
        }
    }
    None
}

/// dmsh_build.vs_tf(src, tf, geometry=False): the transform feedback capture entry point appended to the vertex shader's source `src` (main0 only)
pub fn vs_tf(src: &str, tf: &Value, geometry: bool) -> Result<String, PostError> {
    let Some((pe, pend, body_at)) = main_header(src) else {
        return Err(failed(
            "vs_tf",
            "[RED] vertex shader without a main0 main function, cannot add the transform feedback entry point",
        ));
    };
    let params = &src[pe..pend];
    let body = &src[body_at..];
    let vm = param_named(params, "vertex_id");
    let im = param_named(params, "instance_id");
    let vid = vm.unwrap_or("dm_vid");
    let iid = im.unwrap_or("dm_iid");
    // st = re.search(r'struct main0_out\s*\{(.*?)\};', src, re.S); names = {int(l): nm for nm, l in findall(r'float4 (\w+) \[\[user\(locn(\d+)\)\]\]', group 1)}
    let mut names: Vec<(i64, &str)> = Vec::new();
    if let Some(st) = crate::blend::out_struct(src) {
        let sb = st.as_bytes();
        let mut from = 0;
        while let Some(p) = find_b(sb, b"float4 ", from) {
            from = p + 1;
            let mut c = Cur::at(st, p + 7);
            let Some(nm) = c.word() else { continue };
            if !c.lit(" [[user(locn") {
                continue;
            }
            let Some(d) = c.digits() else { continue };
            if !c.lit(")]]") {
                continue;
            }
            let l: i64 = d.parse().unwrap_or(i64::MAX);
            if let Some(e) = names.iter_mut().find(|(k, _)| *k == l) {
                e.1 = nm;
            } else {
                names.push((l, nm));
            }
            from = c.i;
        }
    }
    let tf_arr: &[Value] = tf.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    let mut stores: Vec<String> = Vec::new();
    for (b, e) in tf_arr.iter().enumerate() {
        if falsy(e) {
            continue;
        }
        let stride = e.get("stride").and_then(|v| v.as_i64()).unwrap_or(0);
        let mut offset = 0i64;
        for out in e
            .get("outputs")
            .and_then(|v| v.as_array())
            .map(|a| a.as_slice())
            .unwrap_or(&[])
        {
            let o = out.get(0).and_then(|v| v.as_i64()).unwrap_or(0);
            let n = out.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
            for (i, c) in "xyzw".chars().take(n.max(0) as usize).enumerate() {
                let value = match names.iter().find(|(k, _)| *k == o) {
                    Some((_, nm)) => format!("as_type<uint>(out.{nm}.{c})"),
                    None => {
                        if c == 'w' {
                            "0x3f800000u".to_string()
                        } else {
                            "0u".to_string()
                        }
                    }
                };
                stores.push(format!(
                    "dm_tf{b}[dm_tf.offset[{b}] / 4 + dm_linear * {} + {}] = {value};",
                    stride.div_euclid(4),
                    offset + i as i64
                ));
            }
            offset += n;
        }
    }
    let mut extra = String::new();
    if vm.is_none() {
        extra.push_str(", uint dm_vid [[vertex_id]]");
    }
    if im.is_none() {
        extra.push_str(", uint dm_iid [[instance_id]]");
    }
    for (b, e) in tf_arr.iter().enumerate() {
        if !falsy(e) {
            extra.push_str(&format!(
                ", device uint* dm_tf{b} [[buffer({})]]",
                TF_SLOT as usize + b
            ));
        }
    }
    extra.push_str(", constant dm_tf_capture& dm_tf [[buffer(14)]]");
    let mut pro = format!(
        "uint dm_linear = ({iid} - dm_tf.base_instance) * dm_tf.count + {vid} - dm_tf.first;\n"
    );
    if geometry {
        pro.push_str(&format!("{vid} = dm_tf.order[dm_linear];\n"));
    }
    let decl = "\nstruct dm_tf_capture { uint4 offset; uint4 args; device const uint* order; uint first, count, base_instance, pad; ulong reserved; };\n";
    let ret = format!(
        "{{ if (dm_linear < dm_tf.args.x) {{ {} }} return; }}",
        stores.join(" ")
    );
    Ok(format!(
        "{decl}\nvertex void main0_tf({params}{extra})\n{{\n{pro}{}",
        body.replace("return out;", &ret)
    ))
}

/// dmsh_build.tf_layout(key, ctl): the four feedback streams of the control section `ctl` (the program record from its control magic on) as the job's `tf` value
/// (a JSON array of four entries, `[]` for an unused stream or `{stride, outputs: [[location - 0x20, components], ...]}`), or None when the program has none or the layout
/// is of an unknown form. The second value is the `[RED] ...` line the Python prints for an unknown form. Err when `ctl` is too short to hold the table (Python: struct.error / IndexError).
pub fn tf_layout(key: &str, ctl: &[u8]) -> Result<(Option<Value>, Option<String>), String> {
    if ctl.len() < 0x6dc + 8 {
        return Err("unpack_from requires a buffer of at least 1766 bytes".into());
    }
    let strides: Vec<u32> = (0..4)
        .map(|b| u16::from_le_bytes([ctl[0x6dc + 2 * b], ctl[0x6dc + 2 * b + 1]]) as u32)
        .collect();
    if strides.iter().all(|&s| s == 0) {
        return Ok((None, None));
    }
    let mut out: Vec<Value> = Vec::new();
    #[allow(clippy::needless_range_loop)] // `b` also indexes `ctl` and offsets the per-buffer reads
    for b in 0..4usize {
        if strides[b] == 0 {
            out.push(Value::Array(vec![]));
            continue;
        }
        let count = *ctl.get(0x6e8 + b).ok_or("index out of range")? as usize;
        let sl = |base: usize| -> Vec<u8> {
            let a = (base + 0x80 * b).min(ctl.len());
            let e = (base + 0x80 * b + count).min(ctl.len());
            ctl[a..e].to_vec()
        };
        let locs = sl(0xdc);
        let sizes = sl(0x4dc);
        let sum: u32 = sizes.iter().map(|&x| x as u32).sum();
        if !(1..=128).contains(&count)
            || locs.iter().any(|&x| !(0x20..0x40).contains(&x))
            || sizes.iter().any(|&n| !(1..=4).contains(&n))
            || strides[b] != 4 * sum
        {
            let l: Vec<String> = locs.iter().map(|x| x.to_string()).collect();
            let s: Vec<String> = sizes.iter().map(|x| x.to_string()).collect();
            return Ok((
                None,
                Some(format!(
                    "[RED] {key}: transform feedback buffer {b} has a layout of unknown form (stride {}, outputs [{}], components [{}]): no capture entry point; see tf_layout",
                    strides[b],
                    l.join(", "),
                    s.join(", ")
                )),
            ));
        }
        let outputs: Vec<Value> = locs
            .iter()
            .zip(sizes.iter())
            .map(|(&o, &n)| serde_json::json!([o as i64 - 0x20, n]))
            .collect();
        out.push(serde_json::json!({"stride": strides[b], "outputs": outputs}));
    }
    Ok((Some(Value::Array(out)), None))
}
