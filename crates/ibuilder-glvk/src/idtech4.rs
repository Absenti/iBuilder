//! `idtech4_programs.py`: id Tech 4's BFG Edition renderer (DOOM 3 on the Switch) makes its GLSL at run time from the render programs of its `.resources`
//! containers (`renderprogs/<name>.vertex` / `.pixel`), the first time a program is used. This finds the renderer's own finders in `main` from its code (not from
//! addresses) and lists every render program, so a capture run of the host (`DM_GL_RENDERPROGS=<job file>`, `hle_gl.m`) can have the game convert each one:
//!
//! * the shader loader: the one function whose code takes the address of its GLSL cache path `renderprogs\glsl\%s` (adrp + add);
//! * the per-stage loaders: its two callers, told apart by the stage they pass (`mov w1, #GL_VERTEX_SHADER 0x8b31` / `#GL_FRAGMENT_SHADER 0x8b30`);
//! * the finders (`FindVertexShader` / `FindFragmentShader`): the callers of one per-stage loader that do not call the other;
//! * the manager: what the callers of a finder load into x0 from main's data (adrp x0 + ldr x0, [x0, #off]), the slot's offset.
//!
//! Job file: `renderprogs <find vertex> <find fragment> <manager slot>` (hex offsets in main), then `v <name>` / `f <name>` per render program.
//! The job is only an input of a capture BOOT of the game (the host converts the programs; `gl_shaders` then translates the captured GLSL); a phone has no such boot yet.

use crate::walk;
use crate::{Fatal, red};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const CACHE_PATH: &[u8] = b"renderprogs\\glsl\\%s\0";
pub const GL_VERTEX_SHADER: u32 = 0x8b31;
pub const GL_FRAGMENT_SHADER: u32 = 0x8b30;

/// What [`find`] found (offsets in main)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Finders {
    pub find_vertex: i64,
    pub find_fragment: i64,
    pub manager_slot: i64,
}

fn u32_at(img: &[u8], o: i64) -> Result<u32, Fatal> {
    // `struct.unpack_from('<I', img, o)`: a negative offset counts from the end, as Python's does
    let o = if o < 0 { img.len() as i64 + o } else { o };
    if o < 0 || o as usize + 4 > img.len() {
        return red(format!(
            "struct.error: unpack_from requires a buffer of at least {} bytes",
            o + 4
        ));
    }
    let o = o as usize;
    Ok(u32::from_le_bytes([
        img[o],
        img[o + 1],
        img[o + 2],
        img[o + 3],
    ]))
}

fn i32_at(img: &[u8], o: i64) -> Result<i64, Fatal> {
    Ok(u32_at(img, o)? as i32 as i64)
}

/// `func_starts(img)`: function starts from the module's `.eh_frame_hdr` (through MOD0)
pub fn func_starts(img: &[u8]) -> Result<Vec<i64>, Fatal> {
    let m0 = u32_at(img, 4)? as i64;
    if img.get(m0 as usize..m0 as usize + 4) != Some(b"MOD0".as_slice()) {
        return Ok(Vec::new());
    }
    let ehs = m0 + i32_at(img, m0 + 16)?;
    let n = i32_at(img, ehs + 8)?;
    let mut out = Vec::new();
    for i in 0..n.max(0) {
        out.push(ehs + i32_at(img, ehs + 12 + 8 * i)?);
    }
    out.sort();
    Ok(out)
}

type Scan = (Vec<(i64, i64)>, Vec<(i64, i64)>, BTreeMap<i64, i64>);

fn sext(v: i64, bits: u32) -> i64 {
    if v & (1 << (bits - 1)) != 0 {
        v - (1 << bits)
    } else {
        v
    }
}

/// `scan(img, text)`: bl targets, string references (adrp + add) and pointer loads into x0 (adrp x0 + ldr x0, [x0, #off]) of the text segment
pub fn scan(img: &[u8], text: (u32, u32)) -> Result<Scan, Fatal> {
    let (mut bls, mut adds, mut loads) = (Vec::new(), Vec::new(), BTreeMap::new());
    let mut page: [Option<(i64, i64)>; 32] = [None; 32];
    let (a, n) = (text.0 as i64, text.1 as i64);
    let mut o = a;
    while o < a + n {
        let w = u32_at(img, o)? as i64;
        if (w & 0xFC000000) == 0x94000000 {
            bls.push((o, o + sext(w & 0x3FFFFFF, 26) * 4));
        } else if (w & 0x9F000000) == 0x90000000 {
            let imm = ((w >> 29) & 3) | (((w >> 5) & 0x7FFFF) << 2);
            page[(w & 31) as usize] = Some(((o & !0xFFF) + (sext(imm, 21) << 12), o));
        } else if (w & 0xFFC00000) == 0x91000000 {
            if let Some((p, at)) = page[((w >> 5) & 31) as usize]
                && o - at <= 16
            {
                adds.push((o, p + ((w >> 10) & 0xFFF)));
            }
        } else if (w & 0xFFC003FF) == 0xF9400000
            && let Some((p, at)) = page[0]
            && o - at <= 8
        {
            loads.insert(o, p + ((w >> 10) & 0xFFF) * 8);
        }
        o += 4;
    }
    Ok((bls, adds, loads))
}

/// `find(img, text)`: the finders and the manager slot, or `None` when main has no BFG render program manager
pub fn find(img: &[u8], text: (u32, u32)) -> Result<Option<Finders>, Fatal> {
    let mut needle = vec![0u8];
    needle.extend_from_slice(CACHE_PATH);
    let Some(at) = img.windows(needle.len()).position(|w| w == needle) else {
        return Ok(None);
    };
    let at = at as i64 + 1;
    let starts = func_starts(img)?;
    if starts.is_empty() {
        return Ok(None);
    }
    // starts[bisect_right(starts, o) - 1], with Python's index -1 when o is before the first start
    let fun = |o: i64| -> i64 {
        let k = starts.partition_point(|&s| s <= o);
        if k == 0 {
            *starts.last().unwrap()
        } else {
            starts[k - 1]
        }
    };
    let (bls, adds, loads) = scan(img, text)?;
    let loader: BTreeSet<i64> = adds
        .iter()
        .filter(|&&(_, t)| t == at)
        .map(|&(o, _)| fun(o))
        .collect();
    if loader.len() != 1 {
        return Ok(None);
    }
    let loader = *loader.iter().next().unwrap();
    // per_stage: insertion-ordered as the Python's dict
    let mut per_stage: Vec<(u32, i64)> = Vec::new();
    for &(o, t) in &bls {
        if t != loader {
            continue;
        }
        let mut back = 4;
        while back < 40 {
            let w = u32_at(img, o - back)?;
            if (w & 0xFFE0001F) == 0x52800001 {
                let st = (w >> 5) & 0xFFFF;
                let f = fun(o);
                match per_stage.iter_mut().find(|(s, _)| *s == st) {
                    Some(e) => e.1 = f,
                    None => per_stage.push((st, f)),
                }
                break;
            }
            back += 4;
        }
    }
    let stages: BTreeSet<u32> = per_stage.iter().map(|&(s, _)| s).collect();
    if stages != BTreeSet::from([GL_VERTEX_SHADER, GL_FRAGMENT_SHADER]) {
        return Ok(None);
    }
    // calls: function -> the per-stage loaders it calls (insertion order of first call)
    let mut calls: Vec<(i64, BTreeSet<u32>)> = Vec::new();
    for &(o, t) in &bls {
        for &(st, f) in &per_stage {
            if t == f {
                let c = fun(o);
                match calls.iter_mut().find(|(k, _)| *k == c) {
                    Some(e) => {
                        e.1.insert(st);
                    }
                    None => calls.push((c, BTreeSet::from([st]))),
                }
            }
        }
    }
    let mut finders: Vec<(u32, i64)> = Vec::new();
    for &(st, _) in &per_stage {
        let only: Vec<i64> = calls
            .iter()
            .filter(|(_, s)| s.len() == 1 && s.contains(&st))
            .map(|(f, _)| *f)
            .collect();
        if only.len() != 1 {
            return Ok(None);
        }
        finders.push((st, only[0]));
    }
    // collections.Counter(...).most_common(1): the highest count, the first counted among equals
    let mut slots: Vec<(i64, usize)> = Vec::new();
    for &(o, t) in &bls {
        if finders.iter().any(|&(_, f)| f == t)
            && let Some(&slot) = loads.get(&(o - 4))
        {
            match slots.iter_mut().find(|(s, _)| *s == slot) {
                Some(e) => e.1 += 1,
                None => slots.push((slot, 1)),
            }
        }
    }
    let Some(best) = slots.iter().map(|&(_, n)| n).max() else {
        return Ok(None);
    };
    let slot = slots.iter().find(|&&(_, n)| n == best).unwrap().0;
    let get = |st: u32| finders.iter().find(|&&(s, _)| s == st).unwrap().1;
    Ok(Some(Finders {
        find_vertex: get(GL_VERTEX_SHADER),
        find_fragment: get(GL_FRAGMENT_SHADER),
        manager_slot: slot,
    }))
}

fn be32(b: &[u8], o: usize) -> Result<u32, Fatal> {
    match b.get(o..o + 4) {
        Some(x) => Ok(u32::from_be_bytes([x[0], x[1], x[2], x[3]])),
        None => red("struct.error: a container table is truncated"),
    }
}

/// `container_names(data)`: the entry names of an id `.resources` container (big-endian table: magic 0xD000000D, table offset and length; per entry a little-endian
/// name length, the name, big-endian offset and length); `[]` when the magic is another
pub fn container_names(data: &[u8]) -> Result<Vec<String>, Fatal> {
    let magic = be32(data, 0)?;
    let (toff, tlen) = (be32(data, 4)? as usize, be32(data, 8)? as usize);
    if magic != 0xD000000D {
        return Ok(Vec::new());
    }
    let t = data
        .get(toff.min(data.len())..(toff.saturating_add(tlen)).min(data.len()))
        .unwrap_or(&[]);
    let n = be32(t, 0)?;
    let (mut p, mut out) = (4usize, Vec::new());
    for _ in 0..n {
        let ln = match t.get(p..p + 4) {
            Some(x) => u32::from_le_bytes([x[0], x[1], x[2], x[3]]) as usize,
            None => return red("struct.error: a container table is truncated"),
        };
        let name = t.get(p + 4..(p + 4 + ln).min(t.len())).unwrap_or(&[]);
        out.push(crate::py::latin1(name));
        p += 4 + ln + 8;
    }
    Ok(out)
}

/// `renderprogs/<name>.vertex` / `.pixel` (`re.fullmatch(r'renderprogs/([^/]+)\.(vertex|pixel)', name)`): (name, is vertex)
fn render_program(name: &str) -> Option<(&str, bool)> {
    let rest = name.strip_prefix("renderprogs/")?;
    if rest.contains('/') {
        return None;
    }
    if let Some(n) = rest.strip_suffix(".vertex")
        && !n.is_empty()
    {
        return Some((n, true));
    }
    if let Some(n) = rest.strip_suffix(".pixel")
        && !n.is_empty()
    {
        return Some((n, false));
    }
    None
}

/// `programs(romfs)`: (vertex names, fragment names) of every container under the RomFS's `base/`, sorted
pub fn programs(romfs: &Path) -> Result<(Vec<String>, Vec<String>), Fatal> {
    let (mut vs, mut fs) = (BTreeSet::new(), BTreeSet::new());
    for d in walk::walk(romfs)? {
        if d.rel != "base" {
            continue;
        }
        for f in &d.files {
            if !f.name.ends_with(".resources") {
                continue;
            }
            let data = f
                .read()
                .map_err(|e| Fatal(format!("[RED] {}: {e}", f.rel)))?;
            for name in container_names(&data)? {
                if let Some((n, v)) = render_program(&name) {
                    if v { &mut vs } else { &mut fs }.insert(n.to_string());
                }
            }
        }
    }
    Ok((vs.into_iter().collect(), fs.into_iter().collect()))
}

/// The job file's text from what [`find`] and [`programs`] gave
pub fn job_text(f: &Finders, vs: &[String], fs: &[String]) -> String {
    let hex = |v: i64| {
        if v < 0 {
            format!("-{:#x}", -v)
        } else {
            format!("{v:#x}")
        }
    };
    let mut s = format!(
        "renderprogs {} {} {}\n",
        hex(f.find_vertex),
        hex(f.find_fragment),
        hex(f.manager_slot)
    );
    for n in vs {
        s += &format!("v {n}\n");
    }
    for n in fs {
        s += &format!("f {n}\n");
    }
    s
}

/// `job(main, romfs)`: the job file's text, or `None` when main has no BFG render program manager or the RomFS no render program
pub fn job(main: &Path, romfs: &Path) -> Result<Option<String>, Fatal> {
    let data = std::fs::read(main).map_err(|e| Fatal(format!("[RED] {}: {e}", main.display())))?;
    let m = ibuilder_nso::load(&data, &main.display().to_string())
        .map_err(|e| Fatal(format!("[RED] {}", e.0)))?;
    let Some(found) = find(&m.image, m.segments[0])? else {
        return Ok(None);
    };
    let (vs, fs) = programs(romfs)?;
    if vs.is_empty() {
        return Ok(None);
    }
    Ok(Some(job_text(&found, &vs, &fs)))
}

/// The line the Python prints after writing the job file
pub fn summary(text: &str) -> String {
    let lines = crate::py::splitlines(text);
    let v = lines.iter().filter(|l| l.starts_with("v ")).count();
    let f = lines.iter().filter(|l| l.starts_with("f ")).count();
    format!(
        "{} ({v} vertex, {f} fragment)",
        lines.first().copied().unwrap_or("")
    )
}

#[cfg(test)]
mod tests {
    //! The engine's own `test_idtech4_programs.py`, ported: a synthetic module laid out as DOOM 3's main is, and a synthetic container. No game files.
    use super::*;

    const LOADER: i64 = 0x100;
    const LV: i64 = 0x140;
    const LF: i64 = 0x180;
    const INIT: i64 = 0x1c0;
    const FINDV: i64 = 0x200;
    const FINDF: i64 = 0x240;
    const PARSER: i64 = 0x280;
    const END: i64 = 0x2c0;
    const MOD0: usize = 0x2000;
    const EHS: usize = 0x2100;
    const STRING: i64 = 0x3001;
    const SLOT: i64 = 0x4018;

    fn adrp(pc: i64, rd: u32, target: i64) -> u32 {
        let imm = ((target & !0xFFF) - (pc & !0xFFF)) >> 12;
        0x90000000 | (((imm & 3) as u32) << 29) | ((((imm >> 2) & 0x7FFFF) as u32) << 5) | rd
    }
    fn add(rd: u32, rn: u32, imm: u32) -> u32 {
        0x91000000 | (imm << 10) | (rn << 5) | rd
    }
    fn movz_w1(imm: u32) -> u32 {
        0x52800001 | (imm << 5)
    }
    fn bl(pc: i64, t: i64) -> u32 {
        0x94000000 | ((((t - pc) >> 2) & 0x3FFFFFF) as u32)
    }
    fn ldr_x0(off: i64) -> u32 {
        0xF9400000 | (((off / 8) as u32) << 10)
    }
    fn put(img: &mut [u8], at: usize, w: u32) {
        img[at..at + 4].copy_from_slice(&w.to_le_bytes());
    }

    fn image(cache: &[u8], stages: (u32, u32)) -> Vec<u8> {
        let mut img = vec![0u8; 0x5000];
        let code: Vec<(i64, Vec<u32>)> = vec![
            (
                LOADER,
                vec![adrp(LOADER, 1, STRING), add(1, 1, (STRING & 0xFFF) as u32)],
            ),
            (LV, vec![movz_w1(stages.0), bl(LV + 4, LOADER)]),
            (LF, vec![movz_w1(stages.1), bl(LF + 4, LOADER)]),
            (INIT, vec![bl(INIT, LV), bl(INIT + 4, LF)]),
            (FINDV, vec![bl(FINDV, LV)]),
            (FINDF, vec![bl(FINDF, LF)]),
            (
                PARSER,
                vec![
                    adrp(PARSER, 0, SLOT),
                    ldr_x0(SLOT & 0xFFF),
                    bl(PARSER + 8, FINDV),
                    adrp(PARSER + 12, 0, SLOT),
                    ldr_x0(SLOT & 0xFFF),
                    bl(PARSER + 20, FINDF),
                ],
            ),
        ];
        for (at, ws) in code {
            for (i, w) in ws.into_iter().enumerate() {
                put(&mut img, at as usize + 4 * i, w);
            }
        }
        put(&mut img, 4, MOD0 as u32);
        img[MOD0..MOD0 + 4].copy_from_slice(b"MOD0");
        put(&mut img, MOD0 + 16, (EHS - MOD0) as u32);
        let fns = [LOADER, LV, LF, INIT, FINDV, FINDF, PARSER, END];
        put(&mut img, EHS + 8, fns.len() as u32);
        for (i, f) in fns.iter().enumerate() {
            put(&mut img, EHS + 12 + 8 * i, (*f - EHS as i64) as i32 as u32);
        }
        img[STRING as usize..STRING as usize + cache.len()].copy_from_slice(cache);
        img
    }

    #[test]
    fn finders_and_manager() {
        let f = find(&image(CACHE_PATH, (0x8b31, 0x8b30)), (0, 0x300)).unwrap();
        assert_eq!(
            f,
            Some(Finders {
                find_vertex: FINDV,
                find_fragment: FINDF,
                manager_slot: SLOT
            })
        );
        assert_eq!(
            job_text(
                &f.unwrap(),
                &["interaction".into()],
                &["interaction".into()]
            ),
            "renderprogs 0x200 0x240 0x4018\nv interaction\nf interaction\n"
        );
    }

    #[test]
    fn stages_swapped() {
        let f = find(&image(CACHE_PATH, (0x8b30, 0x8b31)), (0, 0x300)).unwrap();
        assert_eq!(
            f,
            Some(Finders {
                find_vertex: FINDF,
                find_fragment: FINDV,
                manager_slot: SLOT
            })
        );
    }

    #[test]
    fn another_engine() {
        assert_eq!(
            find(&image(b"shaders\\glsl\\%s\0", (0x8b31, 0x8b30)), (0, 0x300)).unwrap(),
            None
        );
        assert_eq!(
            find(&image(CACHE_PATH, (0x8b31, 0x8b31)), (0, 0x300)).unwrap(),
            None
        );
    }

    #[test]
    fn render_programs_of_the_containers() {
        let names = [
            "renderprogs/interaction.vertex",
            "renderprogs/interaction.pixel",
            "renderprogs/shadow.vertex",
            "renderprogs/global.inc",
            "renderprogs/hlsl/interaction.v360",
            "materials/doom.mtr",
        ];
        let mut table = (names.len() as u32).to_be_bytes().to_vec();
        for n in names {
            table.extend_from_slice(&(n.len() as u32).to_le_bytes());
            table.extend_from_slice(n.as_bytes());
            table.extend_from_slice(&[0u8; 8]);
        }
        let d = std::env::temp_dir().join(format!("glvk-idtech4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("base")).unwrap();
        let mut c = Vec::new();
        for w in [0xD000000Du32, 12, table.len() as u32] {
            c.extend_from_slice(&w.to_be_bytes());
        }
        c.extend_from_slice(&table);
        std::fs::write(d.join("base/_common.resources"), c).unwrap();
        let mut other = Vec::new();
        for w in [0x12345678u32, 12, 0] {
            other.extend_from_slice(&w.to_be_bytes());
        }
        std::fs::write(d.join("base/other.resources"), other).unwrap();
        let (vs, fs) = programs(&d).unwrap();
        assert_eq!(vs, ["interaction", "shadow"]);
        assert_eq!(fs, ["interaction"]);
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(
            summary("renderprogs 0x1 0x2 0x3\nv a\nv b\nf a\n"),
            "renderprogs 0x1 0x2 0x3 (2 vertex, 1 fragment)"
        );
    }
}
