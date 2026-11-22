//! Control sections, code areas and the shader key: the constants and pure functions at the top of `shader_split.py` (l.77-151, 184-186, 377-381).
//!
//! * a **control section** starts with `0x98761234`; a graphics **code area** with the header magic `0x12345678`, a compute one with `0x12345679`
//!   (or none: SDK 10 and later);
//! * the **key** of a shader is `sha256(key_ctl(ctl) + code)[:16]`, the same as the host's `prog_get` ([`skey`]);
//! * the stage comes from the SPH word 0 ([`sph_stage`]), from the NVN2 control section's stage byte ([`nvn2_stage`]) or a GLSLC output's own rules ([`glslc_stage`]).

use crate::bytes::*;
use crate::error::Res;
use sha2::{Digest, Sha256};

pub const CTL_MAGIC: [u8; 4] = [0x34, 0x12, 0x76, 0x98];
pub const GFX_MAGIC: [u8; 4] = [0x78, 0x56, 0x34, 0x12];
pub const CS_MAGIC: [u8; 4] = [0x79, 0x56, 0x34, 0x12];
pub const GLSLC_MAGIC: [u8; 4] = [0x91, 0x68, 0x86, 0x19];
/// `NVN2_KEY_CTL`: the bytes of an NVN2 control section the key hashes
pub const NVN2_KEY_CTL: usize = 0x48;

/// The stage of a shader (`STAGES`, `STAGE_NAME`)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum St {
    Vs,
    Hs,
    Ds,
    Gs,
    Ps,
    Cs,
    Ts,
    Ms,
    /// NVN2 ray tracing (control +0x56 = 8; the rebased engine's `'rt'`)
    Rt,
}

impl St {
    /// `'vs'`, `'hs'`, ... the two-letter name (package file names, the Python's stage strings)
    pub fn short(self) -> &'static str {
        match self {
            St::Vs => "vs",
            St::Hs => "hs",
            St::Ds => "ds",
            St::Gs => "gs",
            St::Ps => "ps",
            St::Cs => "cs",
            St::Ts => "ts",
            St::Ms => "ms",
            St::Rt => "rt",
        }
    }
    /// `STAGE_NAME[st]`: column 3 of `shader_map.tsv`
    pub fn name(self) -> &'static str {
        match self {
            St::Vs => "vertex",
            St::Hs => "hull",
            St::Ds => "domain",
            St::Gs => "geometry",
            St::Ps => "pixel",
            St::Cs => "compute",
            St::Ts => "task",
            St::Ms => "mesh",
            St::Rt => "raytracing",
        }
    }
    pub fn from_short(s: &str) -> Option<St> {
        Some(match s {
            "vs" => St::Vs,
            "hs" => St::Hs,
            "ds" => St::Ds,
            "gs" => St::Gs,
            "ps" => St::Ps,
            "cs" => St::Cs,
            "ts" => St::Ts,
            "ms" => St::Ms,
            "rt" => St::Rt,
            _ => return None,
        })
    }
}

/// `STAGES`, BNSH order
pub const STAGES: [St; 6] = [St::Vs, St::Hs, St::Ds, St::Gs, St::Ps, St::Cs];
/// `NV_STAGES`, the order of a Unity program's header
pub const NV_STAGES: [St; 6] = [St::Vs, St::Ps, St::Gs, St::Hs, St::Ds, St::Cs];

/// `STAGE_NAME` as the Python's dict is used by `shader_device.live_package` (`st not in ss.STAGE_NAME`, `ss.STAGE_NAME[st]`)
pub fn stage_name(short: &str) -> Option<&'static str> {
    St::from_short(short).map(St::name)
}

/// The profile's `shader_key_skip` (`KEY_SKIP`): the control bytes the keys hash as zeros, `offset+length`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct KeyRules {
    pub skip_off: usize,
    pub skip_len: usize,
}

impl KeyRules {
    /// `tuple(int(x, 0) for x in text.split('+'))` (`0x770+0x78`)
    pub fn parse(text: &str) -> Result<KeyRules, String> {
        let parts: Vec<&str> = text.split('+').collect();
        if parts.len() != 2 {
            return Err(format!("shader_key_skip {text:?} is not offset+length"));
        }
        // dm/games/game.py takes only unsigned digits here (`0x[0-9a-fA-F]+|\d+`): a negative would wrap to an offset near usize::MAX
        let n = |s: &str| {
            let v = py_int0(s)
                .ok_or_else(|| format!("shader_key_skip {text:?}: {s:?} is not an integer"))?;
            usize::try_from(v).map_err(|_| format!("shader_key_skip {text:?}: {s:?} is negative"))
        };
        Ok(KeyRules {
            skip_off: n(parts[0])?,
            skip_len: n(parts[1])?,
        })
    }
}

/// Python's `int(s, 0)` for the forms profiles use: decimal, 0x, 0o, 0b, optional sign and underscores between digits
pub fn py_int0(s: &str) -> Option<i64> {
    let t = s.trim();
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (radix, digits) = if let Some(r) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        (16, r)
    } else if let Some(r) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        (8, r)
    } else if let Some(r) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        (2, r)
    } else {
        if t.len() > 1 && t.starts_with('0') && t.chars().any(|c| c != '0' && c != '_') {
            return None; // base 0 does not take leading zeros on a non-zero decimal
        }
        (10, t)
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let clean: String = digits.chars().filter(|&c| c != '_').collect();
    let v = i64::from_str_radix(&clean, radix).ok()?;
    Some(if neg { -v } else { v })
}

/// `sph_stage(code)`: a graphics code area's stage from its SPH word 0 (shader type in bits 10..13; a GLSLC that writes 0 there in a vertex shader is read as one, too)
pub fn sph_stage(code: &[u8]) -> Option<St> {
    if code.len() < 0x34 {
        return None;
    }
    let w = u32::from_le_bytes(code[0x30..0x34].try_into().unwrap());
    if (w >> 10) & 0xf == 0 && w & 0x1f == 1 {
        return Some(St::Vs);
    }
    match (w >> 10) & 0xf {
        1 => Some(St::Vs),
        2 => Some(St::Hs),
        3 => Some(St::Ds),
        4 => Some(St::Gs),
        5 => Some(St::Ps),
        _ => None,
    }
}

/// `nvn2_ctl(ctl)`: an NVN2 (Switch 2) control section: version 0xc9 at +4
pub fn nvn2_ctl(ctl: &[u8]) -> bool {
    ctl.len() >= NVN2_KEY_CTL && ctl[..4] == CTL_MAGIC && ctl[4..8] == [0xc9, 0, 0, 0]
}

/// `ctl_len(ctl)`: the number of control bytes in the key
pub fn ctl_len(ctl: &[u8]) -> Res<usize> {
    if nvn2_ctl(ctl) {
        return Ok(NVN2_KEY_CTL);
    }
    let n = u32le(ctl, 0x20)? as usize;
    Ok(if (0x760..0x880).contains(&n) {
        n
    } else {
        0x880
    })
}

/// `key_ctl(ctl)`: the control bytes in the key: `ctl_len` of them, with the profile's `shader_key_skip` range as zeros (an NVN2 section: +0x30 as zero)
pub fn key_ctl(rules: &KeyRules, ctl: &[u8]) -> Res<Vec<u8>> {
    let n = ctl_len(ctl)?;
    let mut c = ctl[..n.min(ctl.len())].to_vec(); // `ctl[:n]`
    if nvn2_ctl(ctl) {
        // c[:0x30] + bytes(4) + c[0x34:]
        for b in c.iter_mut().take(0x34).skip(0x30) {
            *b = 0;
        }
        return Ok(c);
    }
    let (o, l) = (rules.skip_off, rules.skip_len);
    // `o + l <= len(c)` in Python's unbounded ints: a sum that overflows is past the end
    if let Some(e) = o.checked_add(l).filter(|&e| l != 0 && e <= c.len()) {
        for b in &mut c[o..e] {
            *b = 0;
        }
    }
    Ok(c)
}

/// `skey(ctl, code)`: the 16 hex digits of `sha256(key_ctl + code)`
pub fn skey(rules: &KeyRules, ctl: &[u8], code: &[u8]) -> Res<String> {
    let mut h = Sha256::new();
    h.update(key_ctl(rules, ctl)?);
    h.update(code);
    Ok(hex(&h.finalize()[..8]))
}

/// `same_shader(a, b)`: two copies of one key hold the same shader (the code areas are equal, and so are the control sections up to their own length)
pub fn same_shader(rules: &KeyRules, a: (&[u8], &[u8]), b: (&[u8], &[u8])) -> Res<bool> {
    let ((ca, ka), (cb, kb)) = (a, b);
    if [ca, cb]
        .iter()
        .any(|c| nvn2_ctl(c) && c.len() > 0x56 && c[0x56] == 8)
    {
        // RT entries, continuations and resources are serialized after the standard key's first 0x48 control bytes: only the declared section length is masked, never the RT metadata
        let masked = |c: &[u8]| {
            let mut v = sl(c, 0, 0x30).to_vec();
            v.extend_from_slice(&[0; 4]);
            v.extend_from_slice(sl(c, 0x34, c.len()));
            v
        };
        return Ok(ka == kb && masked(ca) == masked(cb));
    }
    let mut n = if ca.len() >= 0x24 {
        u32le(ca, 0x20)? as usize
    } else {
        ca.len()
    };
    n = if (0x760..=ca.len().min(cb.len())).contains(&n) {
        n
    } else {
        ca.len().max(cb.len())
    };
    if nvn2_ctl(ca) {
        return Ok(ka == kb && key_ctl(rules, ca)? == key_ctl(rules, cb)?);
    }
    let (o, ln) = (rules.skip_off, rules.skip_len);
    if let Some(e) = o.checked_add(ln).filter(|&e| ln != 0 && e <= n) {
        // ca[:o] + ca[o+ln:n]
        let cut = |c: &[u8]| {
            let mut v = sl(c, 0, o).to_vec();
            v.extend_from_slice(sl(c, e, n));
            v
        };
        let (xa, xb) = (cut(ca), cut(cb));
        let n2 = n - ln;
        return Ok(ka == kb && sl(&xa, 0, n2) == sl(&xb, 0, n2));
    }
    Ok(ka == kb && sl(ca, 0, n) == sl(cb, 0, n))
}

/// A reason with text built at run time (`invalid RT metadata: <InvalidModule>`) as the `&'static str` every other reason is: the messages come from a fixed set (the labels of
/// `dmsh_rt`'s `_require` calls), so interning them leaks a few dozen strings at most
fn intern(s: String) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static SET: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut set = SET.get_or_init(Default::default).lock().unwrap();
    if let Some(&x) = set.get(s.as_str()) {
        return x;
    }
    let leaked: &'static str = Box::leak(s.into_boxed_str());
    set.insert(leaked);
    leaked
}

/// `check(where, st, ctl, code)`: control section and code area consistent; returns the reason if not
pub fn check(st: St, ctl: &[u8], code: &[u8]) -> Res<Option<&'static str>> {
    if ctl.len() < 4 || ctl[..4] != CTL_MAGIC || ctl.len() < 0x24 || ctl.len() < ctl_len(ctl)? {
        return Ok(Some("bad control section"));
    }
    if st == St::Rt {
        if !nvn2_ctl(ctl) || ctl.len() < 0x68 || ctl[0x56] != 8 {
            return Ok(Some("bad NVN2 ray-tracing control section"));
        }
        let (nmeta, pmeta, ncode) = (
            u32le(ctl, 0x5c)? as usize,
            u32le(ctl, 0x60)? as usize,
            u32le(ctl, 0x64)? as usize,
        );
        if ncode != code.len() {
            return Ok(Some("RT control +0x64 does not match the code area length"));
        }
        if nmeta == 0
            || pmeta < 0x68
            || pmeta % 8 != 0
            || pmeta > ctl.len()
            || nmeta > ctl.len() - pmeta
        {
            return Ok(Some("RT metadata range lies outside the control section"));
        }
        if let Err(e) = crate::rt::parse_module(&ctl[pmeta..pmeta + nmeta], code) {
            return Ok(Some(intern(format!("invalid RT metadata: {e}"))));
        }
    }
    if nvn2_ctl(ctl) {
        return Ok(if u32le(ctl, 0x44)? as usize == code.len() {
            None
        } else {
            Some("control +0x44 does not match the code area length")
        });
    }
    let c4 = sl(code, 0, 4);
    // a compute code area may have no header (SDK 10's Super Mario 3D World + Bowser's Fury and D2R: instructions start at +0)
    if (st == St::Cs && (c4 == CS_MAGIC || c4 == GFX_MAGIC) && c4 != CS_MAGIC)
        || (st != St::Cs && c4 != GFX_MAGIC)
    {
        return Ok(Some("code area header magic does not match the stage"));
    }
    if u32le(ctl, 0x704)? as usize != code.len() {
        return Ok(Some("control +0x704 does not match the code area length"));
    }
    Ok(None)
}

/// `nvn2_stage(ctl, code)`: an NVN2 shader's stage: compute, task, mesh and RT by the control section's stage byte (+0x56: 5, 6, 7, 8), else the SPH at the code area's start
pub fn nvn2_stage(ctl: &[u8], code: &[u8]) -> Option<St> {
    if ctl.len() > 0x56 {
        match ctl[0x56] {
            5 => return Some(St::Cs),
            6 => return Some(St::Ts),
            7 => return Some(St::Ms),
            8 => return Some(St::Rt),
            _ => {}
        }
    }
    if code.len() >= 4 {
        let mut v = vec![0u8; 0x30];
        v.extend_from_slice(&code[..4]);
        return sph_stage(&v);
    }
    None
}

/// `glslc_stage(ctl, code)`: a GLSLC output's stage, from its code area's header, or compute when the code has no header and the control's thread group (+0x748) is filled
pub fn glslc_stage(ctl: &[u8], code: &[u8]) -> Res<Option<St>> {
    if nvn2_ctl(ctl) {
        return Ok(nvn2_stage(ctl, code));
    }
    let c4 = sl(code, 0, 4);
    let headerless_cs = c4 != CS_MAGIC && c4 != GFX_MAGIC && ctl.len() >= 0x754 && {
        let (x, y, z) = (u32le(ctl, 0x748)?, u32le(ctl, 0x74c)?, u32le(ctl, 0x750)?);
        x != 0 && y != 0 && z != 0
    };
    Ok(if c4 == CS_MAGIC || headerless_cs {
        Some(St::Cs)
    } else if c4 == GFX_MAGIC && code.len() >= 0x34 {
        sph_stage(code)
    } else {
        None
    })
}

/// The record of one shader in a package (`pkg/<stage>_<group>.bin`): `'BTCDBTCD'` + the key's 8 bytes + padding to 0x38 (the control and code lengths at +0x28 / +0x2c), then
/// the code area, then the control section. `Err` is the Python's `red()` for a record `dmsh_build` would cut wrongly.
pub fn record(key_hex: &str, ctl: &[u8], code: &[u8]) -> Result<Vec<u8>, String> {
    let kb = unhex(key_hex).ok_or_else(|| format!("key {key_hex}: not hex"))?;
    let mut rec = Vec::with_capacity(0x38 + code.len() + ctl.len());
    rec.extend_from_slice(b"BTCDBTCD");
    rec.extend_from_slice(&kb);
    rec.resize(0x38, 0);
    rec[0x28..0x2c].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
    rec[0x2c..0x30].copy_from_slice(&(code.len() as u32).to_le_bytes());
    rec.extend_from_slice(code);
    rec.extend_from_slice(ctl);
    if find(&rec, &CTL_MAGIC, 0x38) != Some(0x38 + code.len()) {
        return Err(format!(
            "key {key_hex}: the control magic appears in the code area, dmsh_build would split the record wrongly"
        ));
    }
    if count_nonoverlapping(&rec, b"BTCDBTCD") != 1 {
        return Err(format!(
            "key {key_hex}: the record separator appears inside the record"
        ));
    }
    Ok(rec)
}

/// The compute shader's facts (`shader_split.main`, `live_package`): the thread group and the shared memory rounded up to 16 bytes. `Err` is `red()` for a thread group of 0.
pub fn compute_facts(ctl: &[u8]) -> Result<([u32; 3], u64), String> {
    let at = if nvn2_ctl(ctl) {
        (0x58usize, 0x64usize)
    } else {
        (0x748, 0x754)
    };
    let g = |o: usize| u32le(ctl, o).map_err(|e| e.to_string());
    let (x, y, z) = (g(at.0)?, g(at.0 + 4)?, g(at.0 + 8)?);
    let sh = g(at.1)?;
    // `x * y * z` is a Python int: no wrap
    if (x as u128) * (y as u128) * (z as u128) == 0 {
        return Err(format!("thread group is 0 (control +{:#x})", at.0));
    }
    Ok(([x, y, z], (sh as u64).div_ceil(16) * 16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_int_base_zero() {
        assert_eq!(py_int0("0x770"), Some(0x770));
        assert_eq!(py_int0("120"), Some(120));
        assert_eq!(py_int0("0b101"), Some(5));
        assert_eq!(py_int0("0o17"), Some(15));
        assert_eq!(py_int0("010"), None);
        assert_eq!(py_int0("0"), Some(0));
        assert_eq!(py_int0("1_000"), Some(1000));
        assert_eq!(
            KeyRules::parse("0x770+0x78").unwrap(),
            KeyRules {
                skip_off: 0x770,
                skip_len: 0x78
            }
        );
        assert!(KeyRules::parse("0x770").is_err());
        // a negative offset or length is refused (it wrapped to usize::MAX before)
        assert!(KeyRules::parse("-1+0x10").is_err());
        assert!(KeyRules::parse("0x770+-8").is_err());
    }

    #[test]
    fn a_skip_range_whose_end_overflows_masks_nothing() {
        // offset + length wraps a usize (KeyRules' fields are public): Python's unbounded sum is past the end, so nothing is masked
        let rules = KeyRules {
            skip_off: usize::MAX,
            skip_len: 0x10,
        };
        let mut ctl = vec![7u8; 0x780];
        ctl[..4].copy_from_slice(&CTL_MAGIC);
        ctl[0x20..0x24].copy_from_slice(&0x780u32.to_le_bytes());
        assert_eq!(key_ctl(&rules, &ctl).unwrap(), ctl);
        let code = [1u8; 8];
        assert!(same_shader(&rules, (&ctl, &code), (&ctl, &code)).unwrap());
        let mut other = ctl.clone();
        other[0x77f] ^= 1;
        assert!(!same_shader(&rules, (&ctl, &code), (&other, &code)).unwrap());
    }

    #[test]
    fn the_record_header_is_the_lengths_after_the_key() {
        let mut ctl = vec![0u8; 0x760];
        ctl[..4].copy_from_slice(&CTL_MAGIC);
        let code = vec![1u8; 0x40];
        let r = record("0011223344556677", &ctl, &code).unwrap();
        assert_eq!(&r[..8], b"BTCDBTCD");
        assert_eq!(&r[8..16], &[0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77]);
        assert_eq!(u32::from_le_bytes(r[0x28..0x2c].try_into().unwrap()), 0x760);
        assert_eq!(u32::from_le_bytes(r[0x2c..0x30].try_into().unwrap()), 0x40);
        assert_eq!(r.len(), 0x38 + 0x40 + 0x760);
        // the magic inside the code area would cut the record wrongly; so would the record separator
        let mut bad = code.clone();
        bad[0x10..0x14].copy_from_slice(&CTL_MAGIC);
        assert!(record("0011223344556677", &ctl, &bad).is_err());
        let mut bad = code.clone();
        bad[0x10..0x18].copy_from_slice(b"BTCDBTCD");
        assert!(record("0011223344556677", &ctl, &bad).is_err());
    }
}
