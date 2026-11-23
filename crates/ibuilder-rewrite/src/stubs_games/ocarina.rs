//! The Legend of Zelda: Ocarina of Time (Switch 2, 0400486024DEC000): `FRAME_STEP` (the v131072 main's build ID) and `MATCH_FRAME_STEP` (the same stubs found by their
//! functions' fingerprints) of `dm/games/0400486024DEC000/patches.py`: the frame ratio from the host's step, the dynamic resolution, and the screen shape stubs
//! (`_camera_aspect`: widescreen / Hor+ for `screen_aspect wide`, the `ANAMORPHIC_ASPECT` capability) that only the build-ID form has.
//! Ported against the patch file of `tools/overlay/` (the pinned engine's plus nxnative c12000da, the dynamic-resolution threshold scaled by 60 / the target fps, and
//! b3d287fe, the render scale stubs of the 3D view-size update).

use crate::Res;
use crate::stubs::{RulePort, StubCtx};
use crate::stubs_games::{GamePort, f32_bits, word_hex};
use ibuilder_a64::asm::{self, b};

pub static PORT: GamePort = GamePort {
    profile: "0400486024DEC000",
    digest: "0a49680646e29fc6",
    frame_step: &[("e0f3ccfaf50065ae4ea01eb5aeb13b1a", frame_step_v131072)],
    stubs: &[],
    match_frame_step: &[(
        "main",
        &[RulePort {
            stub: ocarina_frame_step,
            find: None,
        }],
    )],
    match_stubs: &[],
};

/// `frame_step_v131072`: the registered build's stubs: its three functions at their v131072 offsets, then the screen shape stubs
pub fn frame_step_v131072(ctx: &mut StubCtx) -> Res<()> {
    ctx.base = 0x6e6be0;
    ctx.others = vec![
        ("dynamic resolution", Some(0x8065e0)),
        ("dynamic resolution update", Some(0x805aa0)),
        ("render scale", Some(0x31d3200)),
    ];
    ocarina_frame_step(ctx)?;
    camera_aspect(ctx)
}

/// the frame ratio: the function divides its base rate (60.0, or 30.0 while bit 0 of [x21+0x24f] is set) by the int frame rate at [x21+0x2c8] (fdiv s8,s1,s0 at +0xe4);
/// the stub gives the ratio the host's step instead: step x base / 60, and counts the vsyncs stepped. Until the host has decided a step, and at fps_target 0, the division runs
pub fn ocarina_frame_step(ctx: &mut StubCtx) -> Res<()> {
    let (st, base) = (ctx.state, ctx.base);
    let site = base + 0xe4;
    ctx.site(site, "2818201e")?; // fdiv s8,s1,s0
    let start = ctx.here();
    let w = asm::adrp(16, ctx.here(), st)?;
    ctx.emit(w);
    for w in [
        0xb9400211u32, // ldr w17,[x16]          target fps (0 = off)
        0x340001b1,    // cbz w17,orig (+13)
        0xb9402211,    // ldr w17,[x16,#0x20]    step in vsyncs
        0x34000171,    // cbz w17,orig (+11)     not decided yet
        0x1e230220,    // ucvtf s0,w17
        0xf9401a10,    // ldr x16,[x16,#0x30]
        0x8b110210,    // add x16,x16,x17
    ] {
        ctx.emit(w);
    }
    let w = asm::adrp(17, ctx.here(), st)?;
    ctx.emit(w);
    for w in [
        0xf9001a30u32, // str x16,[x17,#0x30]    vsyncs stepped
        0x1e210808,    // fmul s8,s0,s1          step x base rate
        0x52a84e10,    // mov w16,#0x42700000    60.0
        0x1e270200,    // fmov s0,w16
        0x1e201908,    // fdiv s8,s8,s0          / 60
    ] {
        ctx.emit(w);
    }
    let w = b(ctx.here(), site + 4)?;
    ctx.emit(w);
    ctx.emit(0x1e201828); // orig: fdiv s8,s1,s0; back
    let w = b(ctx.here(), site + 4)?;
    ctx.emit(w);
    let w = b(site, start)?;
    ctx.put(site, &[w], "frame ratio: the host's step \u{d7} base rate / 60 instead of base rate / the game's frame rate (state page target fps; 0 = the game's ratio)");
    ocarina_dynamic_resolution(ctx)?;
    render_scale(ctx)
}

/// the level function of the game's dynamic resolution, held at full resolution (state +0x40 = 0) or climbing on the load measured at each level (DM_DYNAMIC_RESOLUTION).
/// c12000da: the threshold (+0x10) is the game's at 60 fps; under a host target below 60 (state +0, a float: DM_FPS_TARGET 30) it is scaled by 60 / target (83 -> 166)
const DRS_CONTROLLER: [u32; 68] = [
    0xb9404211, 0x35000071, 0x2a1f03e0, 0xd65f03c0, // off (state +0x40 0): level 0
    0xbd401001, 0xbd400202, 0x1e202048, 0x540000ed, 0x52a84e0a, 0x1e270140, 0x1e202040, 0x5400006a,
    0x1e200821, 0x1e221821, // threshold x 60 / the target fps under 60
    0xbd400800, 0xb9417402, 0xb941a003, 0x7100005f, 0x1a9fa042, 0x528001e9, 0x6b09005f, 0x1a89d042,
    0xb9420204, 0x11000484, 0xb9020204, 0xb9420605, 0xb9420a06, 0x6b0200bf, 0x1a8617e6, 0xb9020602,
    0xb9020a06, 0x35000046, 0xb9020e1f, 0x91084207, 0x91094208, 0x710020df, 0x540003cb, 0x1e212000,
    0x540001ad, 0xb9420e0b, 0x1100056b, 0xb9020e0b, 0x7100217f, 0x540002eb, 0xbc2258e0, 0xb8225904,
    0x51000469, 0x1100044a, 0x6b09015f, 0x1a89d140, 0xd65f03c0, 0xb9020e1f, 0xbc2258e0, 0xb8225904,
    0x34000182, 0x51000449, 0xb869590a, 0x340000ea, 0x4b0a008a, 0x7109615f, 0x54000088, 0xbc6958e2,
    0x1e212040, 0x5400006a, 0x2a0903e0, 0xd65f03c0, 0x2a0203e0, 0xd65f03c0,
];

/// b3d287fe: under a host render scale below 1.0 (state +0x10, float bits in 1..0x3f7fffff) level 0, so DM_DYNAMIC_RESOLUTION=1 does not fight DM_RENDER_SCALE;
/// emitted before the controller only when the render scale function was found
const DRS_RENDER_SCALE_GUARD: [u32; 7] = [
    0xb9401211, // ldr w17,[x16,#0x10]   the host render scale's bits
    0x340000d1, // cbz w17,+6            unset: the controller
    0x52a7f002, // mov w2,#0x3f800000    1.0
    0x6b02023f, // cmp w17,w2
    0x54000062, // b.hs +3               1.0 or more, negative or NaN: the controller
    0x2a1f03e0, // mov w0,wzr            level 0
    0xd65f03c0, // ret
];

fn ocarina_dynamic_resolution(ctx: &mut StubCtx) -> Res<()> {
    let st = ctx.state;
    // not both found in this build (a red line from the locator): the game's own dynamic resolution
    let (Some(site), Some(_)) = (
        ctx.other("dynamic resolution"),
        ctx.other("dynamic resolution update"),
    ) else {
        return Ok(());
    };
    ctx.site(site, "017c40bd")?; // ldr s1,[x0,#0x7c]
    let has_render_scale = ctx.other("render scale").is_some();
    let start = ctx.here();
    let w = asm::adrp(16, ctx.here(), st)?;
    ctx.emit(w);
    if has_render_scale {
        for w in DRS_RENDER_SCALE_GUARD {
            ctx.emit(w);
        }
    }
    for w in DRS_CONTROLLER {
        ctx.emit(w);
    }
    let w = b(site, start)?;
    let why = if has_render_scale {
        "dynamic resolution level: 0 (full resolution) also under a render scale below 1.0, or with state +0x40 (DM_DYNAMIC_RESOLUTION) a climb on the load measured at each level"
    } else {
        "dynamic resolution level: 0 (full resolution), or with state +0x40 (DM_DYNAMIC_RESOLUTION) a climb on the load measured at each level"
    };
    ctx.put(site, &[w], why);
    ctx.add_capability("DYNAMIC_RESOLUTION");
    Ok(())
}

// ---- the render scale stubs (b3d287fe): DM_RENDER_SCALE (state +0x10) on the 3D view-size update (v131072 main+0x31d3200) ----

/// `_bcond` / `_cbz_w` of the render scale stubs (offsets inside a stub body, in bytes)
fn rs_bcond(frm: i64, to: i64, cond: u32) -> u32 {
    0x54000000 | ((((to - frm) >> 2) & 0x7ffff) as u32) << 5 | cond
}
fn rs_cbz_w(frm: i64, to: i64, rt: u32) -> u32 {
    0x34000000 | ((((to - frm) >> 2) & 0x7ffff) as u32) << 5 | rt
}

/// `render_scale_status_body`: 19 words, `state_adrp` is word 4 (ADRP x16 to the state page). At the update's answer (+0xce4, ubfx of its status byte) a host scale in
/// (0.0, 1.0) whose last stored value (state +0x290) does not match [x20+0xe5c] turns any status but 1 into 0, so that frame recomputes the 3D sizes
pub fn render_scale_status_body(state_adrp: u32) -> [u32; 19] {
    let at = |k: i64| 4 * k;
    [
        0xd3609c08, // ubfx x8,x0,#32,#8      the original
        0xa9be47f0, // stp x16,x17,[sp,#-32]!
        0xd53b4210, // mrs x16,nzcv
        0xf9000bf0, // str x16,[sp,#16]
        state_adrp, // x16 = state page
        0xb9401211, // ldr w17,[x16,#0x10]    the host scale's bits
        rs_cbz_w(at(6), at(16), 17),
        0x53177e31,                    // lsr w17,w17,#23        sign + exponent
        0x7101fe3f,                    // cmp w17,#0x7f
        rs_bcond(at(9), at(16), 0x2),  // b.hs done
        0xb9429210,                    // ldr w16,[x16,#0x290]   last scaled value stored
        0xb94e5e91,                    // ldr w17,[x20,#0xe5c]   current view scale
        0x6b11021f,                    // cmp w16,w17
        rs_bcond(at(13), at(16), 0x0), // b.eq done
        0x7100051f,                    // cmp w8,#1
        0x1a9f0108,                    // csel w8,w8,wzr,eq      keep status 1; any other -> 0
        0xf9400bf0,                    // done: ldr x16,[sp,#16]
        0xd51b4210,                    // msr nzcv,x16
        0xa8c247f0,                    // ldp x16,x17,[sp],#32
    ]
}

/// `render_scale_store_body`: 16 words, `state_adrp` is word 2. At the store (+0xd48, str s0,[x20,#0xe5c]) the game's scale times the host's below 1.0, recorded at state +0x290
pub fn render_scale_store_body(state_adrp: u32) -> [u32; 16] {
    let at = |k: i64| 4 * k;
    [
        0xadbf7ffe,                   // stp q30,q31,[sp,#-32]!
        0xa9bf47f0,                   // stp x16,x17,[sp,#-16]!
        state_adrp,                   // x16 = state page
        0xd53b4211,                   // mrs x17,nzcv
        0xbd40121e,                   // ldr s30,[x16,#0x10]    the host scale
        0x1e2023c8,                   // fcmp s30,#0.0
        rs_bcond(at(6), at(12), 0xd), // b.le original
        0x1e2e101f,                   // fmov s31,#1.0
        0x1e3f23c0,                   // fcmp s30,s31
        rs_bcond(at(9), at(12), 0xa), // b.ge original
        0x1e3e0800,                   // fmul s0,s0,s30
        0xbd029200,                   // str s0,[x16,#0x290]
        0xbd0e5e80,                   // original: str s0,[x20,#0xe5c]
        0xd51b4211,                   // msr nzcv,x17
        0xa8c147f0,                   // ldp x16,x17,[sp],#16
        0xacc17ffe,                   // ldp q30,q31,[sp],#32
    ]
}

fn render_scale(ctx: &mut StubCtx) -> Res<()> {
    // not found in this build (a red line from the locator): the game's own sizes
    let Some(f) = ctx.other("render scale") else {
        return Ok(());
    };
    let (status, store) = (f + 0xce4, f + 0xd48);
    ctx.site(status, "089c60d3")?; // ubfx x8,x0,#32,#8
    let s = ctx.here();
    for w in render_scale_status_body(asm::adrp(16, s + 4 * 4, ctx.state)?) {
        ctx.emit(w);
    }
    let w = b(ctx.here(), status + 4)?;
    ctx.emit(w);
    let w = b(status, s)?;
    ctx.put(
        status,
        &[w],
        "dynamic-resolution status: a host render scale not yet stored asks for new 3D sizes (state +0x10)",
    );
    ctx.site(store, "805e0ebd")?; // str s0,[x20,#0xe5c]
    let s = ctx.here();
    for w in render_scale_store_body(asm::adrp(16, s + 4 * 2, ctx.state)?) {
        ctx.emit(w);
    }
    let w = b(ctx.here(), store + 4)?;
    ctx.emit(w);
    let w = b(store, s)?;
    ctx.put(
        store,
        &[w],
        "final 3D view scale x the host render scale below 1.0 (1.0 = the game's own)",
    );
    ctx.render_scale = 1;
    Ok(())
}

// ---- the screen shape stubs (`_camera_aspect` and its helpers) ----

const ASPECT_STATE: i64 = 0xa4; // loader.c writes float ow / 1080.0f here when DM_SCREEN_ASPECT >= 1.8 (0 = 16:9)
const ASPECT_ACK: i64 = 0x284;
const PROJECTION_ACK: i64 = 0x288; // one last-applied aspect per camera site

fn addi(rd: u32, rn: u32, imm: u32) -> u32 {
    0x91000000 | ((imm & 0xfff) << 10) | (rn << 5) | rd
}
fn movz(wd: u32, imm: u32) -> u32 {
    0x52800000 | ((imm & 0xffff) << 5) | wd
}
fn movk16(wd: u32, imm: u32) -> u32 {
    0x72a00000 | ((imm & 0xffff) << 5) | wd
}
fn fmov_sw(sd: u32, wn: u32) -> u32 {
    0x1e270000 | (wn << 5) | sd
}
fn fsub(sd: u32, sn: u32, sm: u32) -> u32 {
    0x1e203800 | (sm << 16) | (sn << 5) | sd
}
fn fabs(sd: u32, sn: u32) -> u32 {
    0x1e20c000 | (sn << 5) | sd
}
fn fcmp(sn: u32, sm: u32) -> u32 {
    0x1e202000 | (sm << 16) | (sn << 5)
}
fn ldr_s(st: u32, xn: u32, off: u32) -> u32 {
    0xbd400000 | ((off / 4) << 10) | (xn << 5) | st
}
fn ldar_w(wt: u32, xn: u32) -> u32 {
    0x88dffc00 | (xn << 5) | wt
}
fn stlr_w(wt: u32, xn: u32) -> u32 {
    0x889ffc00 | (xn << 5) | wt
}
fn str_s(st: u32, xn: u32, off: u32) -> u32 {
    0xbd000000 | ((off / 4) << 10) | (xn << 5) | st
}
fn cbz_w(wt: u32, frm: i64, to: i64) -> u32 {
    0x34000000 | ((((to - frm) >> 2) & 0x7ffff) as u32) << 5 | wt
}
fn b_cond(frm: i64, to: i64, cond: u32) -> u32 {
    0x54000000 | ((((to - frm) >> 2) & 0x7ffff) as u32) << 5 | cond
}

fn load_f32(words: &mut Vec<u32>, reg: u32, value: f64) {
    let bits = f32_bits(value);
    words.extend([movz(16, bits), movk16(16, bits >> 16), fmov_sw(reg, 16)]);
}

fn state_addr(ctx: &StubCtx, words: &mut Vec<u32>, reg: u32, off: i64) -> Res<()> {
    let pc = ctx.here() + words.len() as i64 * 4;
    words.extend([
        asm::adrp(reg, pc, ctx.state + off)?,
        addi(reg, reg, ((ctx.state + off) & 0xfff) as u32),
    ]);
    Ok(())
}

fn append_stub(
    ctx: &mut StubCtx,
    site: i64,
    original: usize,
    words: &[u32],
    reason: &str,
) -> Res<()> {
    let start = ctx.here();
    for &w in words {
        ctx.emit(w);
    }
    let w = b(ctx.here(), site + 4 * original as i64)?;
    ctx.emit(w);
    let mut put = vec![b(site, start)?];
    put.extend(std::iter::repeat_n(0xd503201fu32, original - 1));
    ctx.put(site, &put, reason);
    Ok(())
}

fn projection(
    ctx: &mut StubCtx,
    site: i64,
    original: u32,
    base: u32,
    aspect_reg: u32,
    projection_ack: i64,
) -> Res<()> {
    ctx.site(site, &word_hex(original))?;
    let mut w = vec![original];
    load_f32(&mut w, 16, 16.0 / 9.0);
    w.extend([fsub(17, aspect_reg, 16), fabs(17, 17)]);
    load_f32(&mut w, 16, 0.01);
    w.push(fcmp(17, 16));
    let stock = w.len();
    w.push(0);
    state_addr(ctx, &mut w, 17, projection_ack)?;
    w.push(ldar_w(16, 17));
    let no_ack = w.len();
    w.push(0);
    w.push(fmov_sw(16, 16));
    w.extend([fsub(17, aspect_reg, 16), fabs(17, 17)]);
    load_f32(&mut w, 16, 0.01);
    w.push(fcmp(17, 16));
    let stale = w.len();
    w.push(0);
    let apply = w.len();
    state_addr(ctx, &mut w, 17, ASPECT_STATE)?;
    w.push(ldar_w(16, 17));
    let disabled = w.len();
    w.push(0);
    w.push(fmov_sw(aspect_reg, 16));
    w.push(str_s(aspect_reg, base, 176));
    state_addr(ctx, &mut w, 17, ASPECT_ACK)?;
    w.push(stlr_w(16, 17));
    state_addr(ctx, &mut w, 17, projection_ack)?;
    w.push(stlr_w(16, 17));
    let skip_restore = w.len();
    w.push(0);
    let disabled_at = w.len();
    load_f32(&mut w, aspect_reg, 16.0 / 9.0);
    w.push(str_s(aspect_reg, base, 176));
    w.push(0x2a1f03f0);
    state_addr(ctx, &mut w, 17, ASPECT_ACK)?;
    w.push(stlr_w(16, 17));
    state_addr(ctx, &mut w, 17, projection_ack)?;
    w.push(stlr_w(16, 17));
    let end = w.len();
    let h = ctx.here();
    let at = |i: usize| h + i as i64 * 4;
    w[stock] = b_cond(at(stock), at(apply), 3);
    w[no_ack] = cbz_w(16, at(no_ack), at(end));
    w[stale] = b_cond(at(stale), at(end), 2);
    w[disabled] = cbz_w(16, at(disabled), at(disabled_at));
    w[skip_restore] = b(at(skip_restore), at(end))?;
    append_stub(
        ctx,
        site,
        1,
        &w,
        "camera projection: use drawable aspect for 16:9 views, restore stock ratio when disabled",
    )
}

/// `_camera_aspect`: adapted by the engine from craftycorvid/oot-mod (MIT) for 0400486024DEC000 v131072 main
fn camera_aspect(ctx: &mut StubCtx) -> Res<()> {
    for (i, (site, original, base, areg)) in [
        (0x3485cc0i64, 0x2d558400u32, 0u32, 1u32),
        (0x004f1e8, 0x2d558ea0, 21, 3),
        (0x023b9a8, 0x2d558c00, 0, 3),
        (0x033b82c, 0x2d558440, 2, 1),
    ]
    .into_iter()
    .enumerate()
    {
        projection(
            ctx,
            site,
            original,
            base,
            areg,
            PROJECTION_ACK + i as i64 * 4,
        )?;
    }

    // render-view aspect
    let site = 0x025d76ci64;
    let orig = [0x1e2202a1u32, 0x1e221821];
    ctx.site(site, &(word_hex(orig[0]) + &word_hex(orig[1])))?;
    let mut w = orig.to_vec();
    load_f32(&mut w, 16, 16.0 / 9.0);
    w.extend([fsub(17, 1, 16), fabs(17, 17)]);
    load_f32(&mut w, 16, 0.01);
    w.push(fcmp(17, 16));
    let skip_view = w.len();
    w.push(0);
    state_addr(ctx, &mut w, 17, ASPECT_STATE)?;
    w.push(ldar_w(16, 17));
    let skip_off = w.len();
    w.push(0);
    w.push(fmov_sw(1, 16));
    let end = w.len();
    let h = ctx.here();
    w[skip_view] = b_cond(h + skip_view as i64 * 4, h + end as i64 * 4, 2);
    w[skip_off] = cbz_w(16, h + skip_off as i64 * 4, h + end as i64 * 4);
    append_stub(
        ctx,
        site,
        orig.len(),
        &w,
        "render-view aspect: match the widened camera on the output view",
    )?;

    // full-screen 2D layout
    let site = 0x0774368i64;
    let orig_w = 0xad401041u32;
    ctx.site(site, &word_hex(orig_w))?;
    let mut w = vec![
        orig_w,
        0x1e260030,
        movz(17, f32_bits(2.0 / 3840.0)),
        movk16(17, f32_bits(2.0 / 3840.0) >> 16),
        0x6b11021f,
    ];
    let ne_x = w.len();
    w.push(0);
    w.extend([
        0x0e0c3c00 | (4 << 5) | 16,
        0x12007800 | (16 << 5) | 16,
        movz(17, f32_bits(2.0 / 2160.0)),
        movk16(17, f32_bits(2.0 / 2160.0) >> 16),
        0x6b11021f,
    ]);
    let ne_y = w.len();
    w.push(0);
    w.extend([0xd1000000 | (0x1b8 << 10) | 17, 0xf9400000 | (17 << 5) | 16]);
    let mut bg_branches = Vec::new();
    for vt in [0x4bed098i64, 0x4be4fb0, 0x4be7b30, 0x4be61c8] {
        let pc = ctx.here() + w.len() as i64 * 4;
        w.extend([
            asm::adrp(17, pc, vt)?,
            addi(17, 17, (vt & 0xfff) as u32),
            0xeb00001f | (17 << 16) | (16 << 5),
            0x54000000,
        ]);
        bg_branches.push(w.len() - 1);
    }
    state_addr(ctx, &mut w, 17, ASPECT_STATE)?;
    w.push(ldar_w(16, 17));
    let zero = w.len();
    w.push(0);
    let bits = f32_bits(16.0 / 9.0);
    w.extend([
        movz(17, bits),
        movk16(17, bits >> 16),
        fmov_sw(17, 17),
        fmov_sw(18, 16),
        0x1e201800 | (18 << 16) | (17 << 5) | 18,
        0x1e260000 | (18 << 5) | 16,
        0x4e040c00 | (16 << 5) | 17,
        0x6e20dc00 | (17 << 16) | (1 << 5) | 1,
    ]);
    let end = w.len();
    let h = ctx.here();
    let at = |i: usize| h + i as i64 * 4;
    w[ne_x] = b_cond(at(ne_x), at(end), 1);
    w[ne_y] = b_cond(at(ne_y), at(end), 1);
    w[zero] = cbz_w(16, at(zero), at(end));
    for i in bg_branches {
        w[i] = b_cond(at(i), at(end), 0);
    }
    append_stub(
        ctx,
        site,
        1,
        &w,
        "full-screen 2D layout: keep the HUD at 16:9 when the picture fills a wider drawable",
    )?;

    // movie viewport
    let site = 0x3b5c758i64;
    let orig_w = 0xa941a269u32;
    ctx.site(site, &word_hex(orig_w))?;
    let mut w = vec![
        0xd1000000 | (40 << 10) | (29 << 5) | 14,
        ldr_s(22, 14, 8),
        ldr_s(23, 14, 16),
        0x1e203800 | (22 << 16) | (23 << 5) | 18,
    ];
    state_addr(ctx, &mut w, 17, ASPECT_STATE)?;
    w.push(ldar_w(16, 17));
    let no_wide = w.len();
    w.push(0);
    w.push(fmov_sw(19, 16));
    load_f32(&mut w, 20, 16.0 / 9.0);
    w.push(0x1e201800 | (19 << 16) | (20 << 5) | 20);
    load_f32(&mut w, 21, 1.0);
    w.push(fsub(20, 21, 20));
    load_f32(&mut w, 21, 0.5);
    w.extend([
        0x1e200800 | (21 << 16) | (20 << 5) | 20,
        0x1e200800 | (20 << 16) | (18 << 5) | 18,
        0x1e202800 | (18 << 16) | (22 << 5) | 22,
        0x1e203800 | (18 << 16) | (23 << 5) | 23,
        str_s(22, 14, 8),
        str_s(23, 14, 16),
    ]);
    let end = w.len();
    let h = ctx.here();
    w[no_wide] = cbz_w(16, h + no_wide as i64 * 4, h + end as i64 * 4);
    w.push(orig_w);
    append_stub(
        ctx,
        site,
        1,
        &w,
        "movie viewport: letterbox pre-rendered movies within the centred 16:9 HUD",
    )?;

    // speech bubble anchor
    let site = 0x020c2b0i64;
    let orig_w = 0x52a89e08u32;
    ctx.site(site, &word_hex(orig_w))?;
    let mut w = Vec::new();
    state_addr(ctx, &mut w, 17, ASPECT_STATE)?;
    w.push(ldar_w(16, 17));
    let no_wide = w.len();
    w.push(0);
    w.push(fmov_sw(16, 16));
    load_f32(&mut w, 17, 1080.0);
    w.extend([
        0x1e200800 | (17 << 16) | (16 << 5) | 16,
        0x1e260000 | (16 << 5) | 8,
    ]);
    let end = w.len();
    let h = ctx.here();
    w[no_wide] = cbz_w(16, h + no_wide as i64 * 4, h + end as i64 * 4);
    w.push(orig_w);
    append_stub(
        ctx,
        site,
        1,
        &w,
        "speech bubble anchor: keep the bubble over the speaker when the HUD is centred",
    )?;

    ctx.add_capability("ANAMORPHIC_ASPECT");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // the words of tools/overlay/dm/games/0400486024DEC000/patches.py (nxnative c12000da + b3d287fe), printed by the Python with an ADRP word of 0x90000010; upstream's
    // test_ocarina_drs.py runs them natively (it passes on that file: target/agent-work/prof-sync/REPORT.txt)
    #[rustfmt::skip]
    const PY_STATUS: [u32; 19] = [0xd3609c08, 0xa9be47f0, 0xd53b4210, 0xf9000bf0, 0x90000010, 0xb9401211, 0x34000151, 0x53177e31, 0x7101fe3f, 0x540000e2,
        0xb9429210, 0xb94e5e91, 0x6b11021f, 0x54000060, 0x7100051f, 0x1a9f0108, 0xf9400bf0, 0xd51b4210, 0xa8c247f0];
    #[rustfmt::skip]
    const PY_STORE: [u32; 16] = [0xadbf7ffe, 0xa9bf47f0, 0x90000010, 0xd53b4211, 0xbd40121e, 0x1e2023c8, 0x540000cd, 0x1e2e101f, 0x1e3f23c0, 0x5400006a,
        0x1e3e0800, 0xbd029200, 0xbd0e5e80, 0xd51b4211, 0xa8c147f0, 0xacc17ffe];
    #[rustfmt::skip]
    const PY_GUARD: [u32; 7] = [0xb9401211, 0x340000d1, 0x52a7f002, 0x6b02023f, 0x54000062, 0x2a1f03e0, 0xd65f03c0];

    #[test]
    fn render_scale_stub_bodies_are_the_pythons() {
        assert_eq!(render_scale_status_body(0x90000010), PY_STATUS);
        assert_eq!(render_scale_store_body(0x90000010), PY_STORE);
        assert_eq!(DRS_RENDER_SCALE_GUARD, PY_GUARD);
    }

    #[test]
    fn drs_controller_keeps_its_off_prefix_and_scales_the_threshold() {
        // the off check first (state +0x40 0: level 0), then the threshold load, the target fps (state +0, a float) and the 60.0 constant of c12000da
        assert_eq!(
            DRS_CONTROLLER[..4],
            [0xb9404211, 0x35000071, 0x2a1f03e0, 0xd65f03c0]
        );
        assert_eq!(
            DRS_CONTROLLER[4..9],
            [0xbd401001, 0xbd400202, 0x1e202048, 0x540000ed, 0x52a84e0a]
        );
        assert_eq!(DRS_CONTROLLER.len(), 68);
    }
}
