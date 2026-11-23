//! The SPIR-V side of `finish`: `dmsh_build.spv_single_lane` (one-lane subgroups, SPIR-V words rewritten), `dmsh_build.XC` (the spirv-cross arguments),
//! `dmsh_geometry.flatten_sets` (auxiliary descriptor sets into the ordinary set-0 ABI).
use std::collections::HashMap;

/// `dmsh_build.XC`: `dmsh_chain.CROSS_MSL` (MSL 3.2 for every shader) and the padded fragment outputs
pub const XC: [&str; 6] = [
    "--msl",
    "--msl-version",
    "30200",
    "--msl-argument-buffers",
    "--msl-texture-buffer-native",
    "--msl-pad-fragment-output",
];

fn words(d: &[u8]) -> Result<Vec<u32>, String> {
    if !d.len().is_multiple_of(4) {
        return Err("SPIR-V length is not a multiple of 4".into());
    }
    Ok(d.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect())
}
fn bytes(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// The spirv-cross arguments for a module (without the program name and without the files): `XC` for every shader, whatever the module holds
pub fn cross_args(_data: &[u8], _name: &str) -> Result<Vec<String>, String> {
    Ok(XC.iter().map(|s| s.to_string()).collect())
}

fn sg_op(op: u32) -> Option<&'static str> {
    match op {
        333 => Some("true"),
        334 | 335 => Some("pred"),
        336 => Some("true"),
        337 | 338 => Some("value"),
        339 => Some("ballot"),
        345..=348 => Some("value"),
        _ => None,
    }
}
fn sg_builtin(v: u32) -> Option<&'static str> {
    match v {
        36 => Some("one"),
        41 => Some("zero"),
        4416 | 4417 => Some("one4"),
        4418 => Some("zero4"),
        4419 => Some("one4"),
        4420 => Some("zero4"),
        _ => None,
    }
}

fn ep_drop(x: &[u32], ids: &HashMap<u32, &'static str>) -> Vec<u32> {
    let mut k = 3;
    while k < x.len() {
        k += 1;
        if (0..4).any(|b| (x[k - 1] >> (8 * b)) & 0xff == 0) {
            break;
        }
    }
    let mut y: Vec<u32> = x[..k.min(x.len())].to_vec();
    y.extend(x[k.min(x.len())..].iter().filter(|v| !ids.contains_key(v)));
    y[0] = (y.len() as u32) << 16 | (x[0] & 0xffff);
    y
}

/// `spv_single_lane(path)` on the module's bytes -> Some(new bytes) when rewritten, None when there is nothing to do or an operation it does not cover
pub fn spv_single_lane(d: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let words = words(d)?;
    if words.len() < 5 {
        return Ok(None);
    }
    let mut hdr: Vec<u32> = words[..5].to_vec();
    let mut ins: Vec<&[u32]> = Vec::new();
    let mut i = 5;
    while i < words.len() {
        let n = (words[i] >> 16) as usize;
        if n == 0 || i + n > words.len() {
            return Err("invalid SPIR-V instruction".into());
        }
        ins.push(&words[i..i + n]);
        i += n;
    }
    let op = |x: &[u32]| x[0] & 0xffff;
    let g = |x: &[u32], k: usize| x.get(k).copied().unwrap_or(u32::MAX);
    let is_bi = |x: &[u32]| op(x) == 71 && x.len() > 3 && x[2] == 11 && sg_builtin(x[3]).is_some();
    if !ins.iter().any(|x| sg_op(op(x)).is_some() || is_bi(x)) {
        return Ok(None);
    }
    if ins
        .iter()
        .any(|x| (340..=366).contains(&op(x)) && sg_op(op(x)).is_none())
    {
        return Ok(None);
    }
    let mut bvar: HashMap<u32, &'static str> = HashMap::new();
    for x in &ins {
        if is_bi(x)
            && let Some(b) = sg_builtin(x[3])
        {
            bvar.insert(x[1], b);
        }
    }
    let mut bptr: HashMap<u32, (&'static str, u32)> = HashMap::new();
    for x in &ins {
        if (op(x) == 65 || op(x) == 66) && x.len() > 3 && bvar.contains_key(&x[3]) {
            if x.len() != 5 || !bvar[&x[3]].ends_with('4') {
                return Ok(None);
            }
            bptr.insert(x[2], (bvar[&x[3]], x[4]));
        }
    }
    if ins
        .iter()
        .any(|x| (op(x) == 65 || op(x) == 66) && x.len() > 3 && bptr.contains_key(&x[3]))
    {
        return Ok(None);
    }
    let mut uint = ins
        .iter()
        .find(|x| op(x) == 21 && x.len() >= 4 && x[2] == 32 && x[3] == 0)
        .map(|x| x[1]);
    let mut boolt = ins.iter().find(|x| op(x) == 20).map(|x| x[1]);
    let mut uvec4 = uint.and_then(|u| {
        ins.iter()
            .find(|x| op(x) == 23 && x.len() >= 4 && x[2] == u && x[3] == 4)
            .map(|x| x[1])
    });
    // a Python `next(...)` with `uint is None` compares x[2] == None: no vec4 of a missing type
    let mut bound = hdr[3];
    let mut types: Vec<Vec<u32>> = Vec::new();
    if uint.is_none() {
        uint = Some(bound);
        bound += 1;
        types.push(vec![21 | 4 << 16, uint.unwrap(), 32, 0]);
    }
    if boolt.is_none() {
        boolt = Some(bound);
        bound += 1;
        types.push(vec![20 | 2 << 16, boolt.unwrap()]);
    }
    if uvec4.is_none() {
        uvec4 = Some(bound);
        bound += 1;
        types.push(vec![23 | 4 << 16, uvec4.unwrap(), uint.unwrap(), 4]);
    }
    let (uint, boolt, uvec4) = (uint.unwrap(), boolt.unwrap(), uvec4.unwrap());
    let mut c: HashMap<&str, u32> = HashMap::new();
    for nm in ["zero", "one", "zero4", "one4", "true"] {
        c.insert(nm, bound);
        bound += 1;
    }
    let mut consts = types;
    consts.push(vec![43 | 4 << 16, uint, c["zero"], 0]);
    consts.push(vec![43 | 4 << 16, uint, c["one"], 1]);
    consts.push(vec![
        44 | 7 << 16,
        uvec4,
        c["zero4"],
        c["zero"],
        c["zero"],
        c["zero"],
        c["zero"],
    ]);
    consts.push(vec![
        44 | 7 << 16,
        uvec4,
        c["one4"],
        c["one"],
        c["zero"],
        c["zero"],
        c["zero"],
    ]);
    consts.push(vec![41 | 3 << 16, boolt, c["true"]]);
    let mut out: Vec<Vec<u32>> = Vec::new();
    let mut placed = false;
    for x in &ins {
        let o = op(x);
        if o == 17 && matches!(g(x, 1), 61..=68 | 4423 | 4431) {
            continue;
        }
        if o == 10 {
            let b = bytes(&x[1..]);
            if memchr::memmem::find(&b, b"ballot").is_some()
                || memchr::memmem::find(&b, b"subgroup").is_some()
            {
                continue;
            }
        }
        if (o == 71 || o == 5) && bvar.contains_key(&g(x, 1)) {
            continue;
        }
        if o == 59 && bvar.contains_key(&g(x, 2)) {
            continue;
        }
        let mut xv: Vec<u32> = x.to_vec();
        if o == 15 {
            xv = ep_drop(x, &bvar);
        }
        if o == 54 && !placed {
            out.extend(consts.iter().cloned());
            placed = true;
        }
        if o == 61 && bvar.contains_key(&g(x, 3)) {
            out.push(vec![83 | 4 << 16, g(x, 1), g(x, 2), c[bvar[&x[3]]]]);
            continue;
        }
        if (o == 65 || o == 66) && bptr.contains_key(&g(x, 2)) {
            continue;
        }
        if o == 61 && bptr.contains_key(&g(x, 3)) {
            let (nm, index) = bptr[&x[3]];
            out.push(vec![77 | 5 << 16, g(x, 1), g(x, 2), c[nm], index]);
            continue;
        }
        if let Some(kind) = sg_op(o) {
            if kind == "ballot" {
                out.push(vec![
                    169 | 6 << 16,
                    g(x, 1),
                    g(x, 2),
                    g(x, 4),
                    c["one4"],
                    c["zero4"],
                ]);
                continue;
            }
            out.push(vec![
                83 | 4 << 16,
                g(x, 1),
                g(x, 2),
                if kind == "true" { c["true"] } else { g(x, 4) },
            ]);
            continue;
        }
        out.push(xv);
    }
    if !placed {
        return Ok(None);
    }
    hdr[3] = bound;
    let mut flat = hdr;
    for x in out {
        flat.extend(x);
    }
    Ok(Some(bytes(&flat)))
}

/// A recorded set decoration: (id, (set, word index)).
type SetDecl = (u32, (u32, usize));

/// `dmsh_geometry.flatten_sets(path)` on the module's bytes
pub fn flatten_sets(d: &[u8]) -> Result<Vec<u8>, String> {
    let mut w = words(d)?;
    let (mut sets, mut bindings): (Vec<SetDecl>, HashMap<u32, usize>) =
        (Vec::new(), HashMap::new());
    let mut i = 5;
    while i < w.len() {
        let (n, op) = ((w[i] >> 16) as usize, w[i] & 65535);
        if n == 0 || i + n > w.len() {
            return Err("invalid SPIR-V instruction".into());
        }
        if op == 71 && n == 4 {
            if w[i + 2] == 34 {
                let e = (w[i + 3], i + 3);
                if let Some(s) = sets.iter_mut().find(|(id, _)| *id == w[i + 1]) {
                    s.1 = e;
                } else {
                    sets.push((w[i + 1], e));
                }
            }
            if w[i + 2] == 33 {
                bindings.insert(w[i + 1], i + 3);
            }
        }
        i += n;
    }
    for (ident, (s, at)) in sets {
        if s != 0 {
            if (s != 1 && s != 2) || !bindings.contains_key(&ident) {
                return Err("unknown geometry auxiliary descriptor set".into());
            }
            let b = bindings[&ident];
            w[b] = w[b].wrapping_add(if s == 1 { 10000 } else { 11000 });
            w[at] = 0;
        }
    }
    Ok(bytes(&w))
}
