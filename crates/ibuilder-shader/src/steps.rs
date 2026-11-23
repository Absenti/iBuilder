//! The small steps of `finish()`: precise, tg_fence, max_threads, check_msl, raw_noinline, mesh_subgroup_noinline (and membar_fence, which the engine's chain no longer runs:
//! SPIRV-Cross writes MEMBAR's fences itself at MSL 3.2).
use crate::templates as t;
use crate::text::*;
use crate::{PostError, failed};
use std::borrow::Cow;

/// dmsh_atomic.check_msl: fail closed on SPIRV-Cross's pointer emulation of image atomics; 64-bit image atomics (`_rg32ui`) made native (`atomic::wide_atomics`)
pub fn check_msl(src: &str) -> Result<Cow<'_, str>, PostError> {
    let b = src.as_bytes();
    if contains(src, "tomic") && emulated(b) {
        return Err(failed(
            "check_msl",
            "image atomics emulated through a raw pointer; they must be native texture atomics (MSL 3.1)",
        ));
    }
    crate::atomic::wide_atomics(src)
}

/// `\bspvImage2DAtomicCoord\b|device\s+atomic_u?int\s*\*\s*\w+_atomic\b`
fn emulated(b: &[u8]) -> bool {
    let n1 = b"spvImage2DAtomicCoord";
    let mut from = 0;
    while let Some(p) = find_b(b, n1, from) {
        if !in_word(b, p) && !(p + n1.len() < b.len() && is_word(b[p + n1.len()])) {
            return true;
        }
        from = p + 1;
    }
    // the second alternative has no \b before `device`, so every `device` counts
    let mut from = 0;
    while let Some(p) = find_b(b, b"device", from) {
        from = p + 1;
        let mut i = p + 6;
        let s = skip_space(b, i);
        if s == i {
            continue;
        }
        i = s;
        if !starts(b, i, "atomic_") {
            continue;
        }
        i += 7;
        if i < b.len() && b[i] == b'u' {
            i += 1;
        }
        if !starts(b, i, "int") {
            continue;
        }
        i += 3;
        i = skip_space(b, i);
        if i >= b.len() || b[i] != b'*' {
            continue;
        }
        i = skip_space(b, i + 1);
        let e = word_end(b, i);
        // `\w+_atomic\b`: the identifier ends with `_atomic` and has at least one character before it
        if e - i >= 8 && &b[e - 7..e] == b"_atomic" {
            return true;
        }
    }
    false
}

/// dmsh_rewrite.precise: `\bfast::(min|max|clamp)\(` -> `precise::\1(`
pub fn precise(src: &str) -> Cow<'_, str> {
    let b = src.as_bytes();
    let mut out: Option<String> = None;
    let mut at = 0;
    let mut from = 0;
    while let Some(p) = find_b(b, b"fast::", from) {
        from = p + 6;
        if in_word(b, p) {
            continue;
        }
        let q = p + 6;
        if !(starts(b, q, "min(") || starts(b, q, "max(") || starts(b, q, "clamp(")) {
            continue;
        }
        let o = out.get_or_insert_with(|| String::with_capacity(src.len() + 64));
        o.push_str(&src[at..p]);
        o.push_str("precise::");
        at = q;
    }
    match out {
        Some(mut o) => {
            o.push_str(&src[at..]);
            Cow::Owned(o)
        }
        None => Cow::Borrowed(src),
    }
}

/// the 2026-11-16 engine's dmsh_rewrite.membar_fence (no longer a step of `finish()`)
pub fn membar_fence(src: &str) -> Cow<'_, str> {
    if !contains(src, t::MEMBAR) {
        return Cow::Borrowed(src);
    }
    let b = src.as_bytes();
    let m = t::MEMBAR.as_bytes();
    // `^([ \t]*)M\n\1M$` (re.M) -> `\1M`, left to right, non-overlapping
    let mut out = String::with_capacity(src.len());
    let mut i = 0usize; // scan position
    let mut copied = 0usize;
    while i <= b.len() {
        if i == 0 || b[i - 1] == b'\n' {
            let mut k = i;
            while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
                k += 1;
            }
            let ind = &b[i..k];
            if starts(b, k, t::MEMBAR) {
                let mut e = k + m.len();
                if e < b.len() && b[e] == b'\n' {
                    e += 1;
                    if b.len() >= e + ind.len()
                        && &b[e..e + ind.len()] == ind
                        && starts(b, e + ind.len(), t::MEMBAR)
                    {
                        let e2 = e + ind.len() + m.len();
                        if e2 == b.len() || b[e2] == b'\n' {
                            out.push_str(&src[copied..i]);
                            out.push_str(&src[i..k]);
                            out.push_str(t::MEMBAR);
                            copied = e2;
                            i = e2;
                            continue;
                        }
                    }
                }
            }
        }
        i += 1;
    }
    out.push_str(&src[copied..]);
    Cow::Owned(out.replace(t::MEMBAR, t::MEMBAR_FENCE))
}

/// dmsh_rewrite.tg_fence
pub fn tg_fence(src: &str) -> Result<Cow<'_, str>, PostError> {
    let b = src.as_bytes();
    let needle = b"void dmsh_tg_fence()";
    let mut from = 0;
    while let Some(p) = find_b(b, needle, from) {
        from = p + 1;
        if in_word(b, p) {
            continue;
        }
        let q = skip_space(b, p + needle.len());
        if q < b.len() && b[q] == b'{' {
            let open = q; // m.end() - 1
            let end = block_end(b, open).ok_or_else(|| failed("tg_fence", "block not closed"))?;
            let mut o = String::with_capacity(src.len() + 128);
            o.push_str(&src[..open + 1]);
            o.push_str("\n    ");
            o.push_str(t::TG_FENCE);
            o.push('\n');
            o.push_str(&src[end..]);
            return Ok(Cow::Owned(o));
        }
    }
    Ok(Cow::Borrowed(src))
}

/// dmsh_rewrite.max_threads
pub fn max_threads<'a>(src: &'a str, local: Option<&[i64]>) -> Cow<'a, str> {
    let Some(l) = local else {
        return Cow::Borrowed(src);
    };
    if l.len() != 3 || l.iter().any(|&v| v < 1) {
        return Cow::Borrowed(src);
    }
    let n = l[0].saturating_mul(l[1]).saturating_mul(l[2]);
    if n <= 384 || n > 1024 || contains(src, "[[max_total_threads_per_threadgroup(") {
        return Cow::Borrowed(src);
    }
    let b = src.as_bytes();
    let needle = "kernel void main0(";
    let mut from = 0;
    while let Some(p) = find_b(b, needle.as_bytes(), from) {
        if p == 0 || b[p - 1] == b'\n' {
            let mut o = String::with_capacity(src.len() + 64);
            o.push_str(&src[..p]);
            o.push_str(&format!("[[max_total_threads_per_threadgroup({n})]]\n"));
            o.push_str(&src[p..]);
            return Cow::Owned(o);
        }
        from = p + 1;
    }
    Cow::Borrowed(src)
}

/// dmsh_rewrite.raw_noinline: the raw global memory lookup's search (`dmsh_raw_find*`) written `noinline`, one copy the misses branch to; a mesh stage (`ms`) keeps it
/// (or makes it again) always_inline, as Apple's M1 / M2 mesh compiler crashes on some mesh programs that call it. `stage` is the stage HEAD's finish() passes: the mesh kind
/// for a task / mesh shader, else the job's stage.
pub fn raw_noinline<'a>(src: &'a str, stage: &str) -> Cow<'a, str> {
    static RAW_FIND: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"\nstatic (?:inline __attribute__\(\(always_inline\)\)|__attribute__\(\(noinline\)\))\n(\w+ dmsh_raw_find\w*\()",
        )
        .expect("static regex")
    });
    if !contains(src, "dmsh_raw_find") {
        return Cow::Borrowed(src);
    }
    let attr = if stage == "ms" {
        "inline __attribute__((always_inline))"
    } else {
        "__attribute__((noinline))"
    };
    RAW_FIND.replace_all(src, |c: &regex::Captures| {
        format!("\nstatic {attr}\n{}", &c[1])
    })
}

/// dmsh_rewrite.mesh_subgroup_noinline: SPIRV-Cross's helpers that take a subgroup input (`gl_Subgroup*`) out of line in a mesh shader (Apple's mesh backend crashes inlining them),
/// and every `thread T& gl_Subgroup*` parameter passed by value (out of line, the backend reads the lane ID through the reference as one value for the whole SIMD group).
pub fn mesh_subgroup_noinline(src: &str) -> Cow<'_, str> {
    // MESH_SUBGROUP_HELPER `(?m)^static inline __attribute__\(\(always_inline\)\)(?=\n\w+ \w+\([^\n]*\bgl_Subgroup\w*\b)`: the look-ahead is the group after the prefix,
    // kept as it is (its text holds no line start, so no other match can begin inside it)
    static HELPER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?m)^static inline __attribute__\(\(always_inline\)\)(\n\w+ \w+\([^\n]*\bgl_Subgroup)",
        )
        .expect("static regex")
    });
    static REF: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\bthread (\w+)& (gl_Subgroup\w+)\b").expect("static regex")
    });
    let a = HELPER.replace_all(src, "static __attribute__((noinline))$1");
    match REF.replace_all(&a, "$1 $2") {
        Cow::Owned(o) => Cow::Owned(o),
        Cow::Borrowed(_) => match a {
            Cow::Owned(o) => Cow::Owned(o),
            Cow::Borrowed(_) => Cow::Borrowed(src),
        },
    }
}
