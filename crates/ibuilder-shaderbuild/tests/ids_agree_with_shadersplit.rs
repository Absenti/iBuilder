//! `live::ids` (this crate) is a copy of the part of `ibuilder-shadersplit::ctl` that live translation needs: the shader key, the stage of a GLSLC output, the NVN2 test. Two copies of the identity
//! of a shader can drift apart and the library lookup then misses, silently (review finding 4.6). The copy stays (a refactor is not worth the risk), and this test makes a drift fail: it runs
//! both over a deterministic set of control sections and code areas that covers every branch of either, and demands the same answer.

use ibuilder_shaderbuild::live::ids;
use ibuilder_shadersplit::ctl::{self, KeyRules, St};

#[test]
fn the_constants_are_the_same() {
    assert_eq!(ids::CTL_MAGIC, ctl::CTL_MAGIC);
    assert_eq!(ids::GFX_MAGIC, ctl::GFX_MAGIC);
    assert_eq!(ids::CS_MAGIC, ctl::CS_MAGIC);
    assert_eq!(ids::NVN2_KEY_CTL, ctl::NVN2_KEY_CTL);
}

#[test]
fn the_stage_names_are_the_same() {
    for st in [
        "vs", "hs", "ds", "gs", "ps", "cs", "ts", "ms", "", "xx", "VS", "pixel",
    ] {
        assert_eq!(ids::stage_name(st), ctl::stage_name(st), "stage {st:?}");
    }
}

/// xorshift: the cases are the same on every run and every machine
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    if o + 4 <= b.len() {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
}

/// control sections that exercise every branch: the magic or not, NVN2 (version 0xc9) or not, the word at +0x20 in and out of 0x760..0x880, the stage byte at +0x56, the compute thread group
/// at +0x748, every length from "too short for anything" to past 0x880
fn controls(rng: &mut Rng) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for len in [
        0usize, 3, 4, 8, 0x20, 0x24, 0x47, 0x48, 0x49, 0x56, 0x57, 0x100, 0x754, 0x760, 0x780,
        0x880, 0x900,
    ] {
        for kind in 0..6 {
            let mut c = rng.bytes(len);
            if len >= 8 && kind != 0 {
                c[..4].copy_from_slice(&ctl::CTL_MAGIC);
                put32(&mut c, 4, if kind >= 3 { 0xc9 } else { 0x1 });
            }
            put32(
                &mut c,
                0x20,
                [0x0, 0x75f, 0x760, 0x7a0, 0x87f, 0x880, 0x8000][(rng.next() % 7) as usize],
            );
            if c.len() > 0x56 {
                c[0x56] = (rng.next() % 9) as u8;
            }
            if kind == 5 {
                for o in [0x748, 0x74c, 0x750] {
                    put32(&mut c, o, (rng.next() % 3) as u32);
                }
            }
            out.push(c);
        }
    }
    out
}

/// code areas: no header, each magic, SPH words of every shader type at +0x30, lengths around the 0x34 an SPH needs
fn codes(rng: &mut Rng) -> Vec<Vec<u8>> {
    let mut out = vec![Vec::new(), vec![1, 2, 3]];
    for len in [4usize, 0x10, 0x33, 0x34, 0x38, 0x100] {
        for head in 0..4 {
            let mut c = rng.bytes(len);
            match head {
                1 => c[..4].copy_from_slice(&ctl::GFX_MAGIC),
                2 => c[..4].copy_from_slice(&ctl::CS_MAGIC),
                _ => {}
            }
            for ty in 0..7u32 {
                let mut c2 = c.clone();
                put32(&mut c2, 0x30, (ty << 10) | (rng.next() % 2) as u32);
                out.push(c2);
            }
            out.push(c);
        }
    }
    out
}

const SKIPS: [(usize, usize); 6] = [
    (0, 0),
    (0x770, 0x78),
    (0x30, 4),
    (0x10, 0x20),
    (0x870, 0x20),
    (0x1000, 4),
];

#[test]
fn the_key_and_the_stage_agree_on_every_generated_shader() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (ctls, codes) = (controls(&mut rng), codes(&mut rng));
    let mut checked = 0usize;
    for c in &ctls {
        assert_eq!(
            ids::nvn2_ctl(c),
            ctl::nvn2_ctl(c),
            "nvn2_ctl of a {}-byte control",
            c.len()
        );
        // `ctl_len` of a section too short to hold the word at +0x20 is an error in the split and the default in the copy: compared where the split answers
        if let Ok(n) = ctl::ctl_len(c) {
            assert_eq!(ids::ctl_len(c), n, "ctl_len of a {}-byte control", c.len());
        }
        for code in &codes {
            // the stage: the split says `Ok(None)` / `Err` where the copy says `None`
            if let Ok(st) = ctl::glslc_stage(c, code) {
                assert_eq!(
                    ids::glslc_stage(c, code),
                    st.map(St::short),
                    "glslc_stage of a {}-byte control and a {}-byte code area",
                    c.len(),
                    code.len()
                );
                checked += 1;
            }
            for &(o, l) in &SKIPS {
                if let Ok(key) = ctl::skey(
                    &KeyRules {
                        skip_off: o,
                        skip_len: l,
                    },
                    c,
                    code,
                ) {
                    assert_eq!(
                        ids::skey(c, code, (o, l)),
                        key,
                        "skey of a {}-byte control and a {}-byte code area, skip {o:#x}+{l:#x}",
                        c.len(),
                        code.len()
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(
        checked > 20_000,
        "the generated set is too small to mean anything: {checked} comparisons"
    );
}
