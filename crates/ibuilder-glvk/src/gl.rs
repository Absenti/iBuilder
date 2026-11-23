//! `gl_shaders.py`: an OpenGL game's GLSL shaders -> MSL + the GL layer's table (`gl/gl_shaders.tsv`, ABI 2), byte-identical to the Python's.
//!
//! The host's GL layer (`dm/gfx/gl/hle_gl.m`) finds a shader again by `sha256(the GLSL source up to its first NUL)[:16]` and its stage; the Metal function is
//! `gl_<key>_v` / `gl_<key>_f`. Per shader: the GLSL is promoted when it is GLSL ES 1.00 or legacy desktop GLSL, preprocessed (`glslangValidator -E`), fixed up for what
//! NVIDIA's compiler takes and glslang does not, compiled to SPIR-V (`-V -R --aml --amb -q`: relaxed Vulkan rules gather the loose uniforms into
//! `gl_DefaultUniformBlock`; the `-q` reflection names the active uniforms), its bound variables renamed in the SPIR-V, translated by SPIRV-Cross (one Tier 1 argument
//! buffer per stage), and its varyings renamed by name. The table row lists the vertex inputs, uniform members, argument IDs, samplers and fragment outputs.
//!
//! Sources, in every input path: MVGL archives (`<material>_vp.shad` / `_fp.shad`), `.vert` / `.frag` files, `.glsl` files that define `main`, libGDX `.vs` + `.fs`
//! pairs, and, for a file root that is an NSO, every `#version` string of its read-only segment that defines `main`.
//!
//! Not ported: the Mac capture host's `--capture-shader` mode, and the Metal compile (the MSL text is the output, [`Layer::msl`]).
//!
//! Python semantics this port does not reproduce: Python's Unicode `\w` (it counts `²³¹¼½¾` and other numeric characters as word characters, Rust marks and
//! connector punctuation) and `\b`; `\s` is Python's (with `\x1c`-`\x1f`). The sources are latin1 text; GLSL is ASCII in practice. Two variables whose bindings are
//! renamed iterate in ascending id order (the Python iterates a `set` of ints). glslang's stdout is decoded lossily where the Python raises on invalid UTF-8.

use crate::mvgl::Archive;
use crate::tools::{Tool, run};
use crate::walk::{self, Root};
use crate::{Api, Fatal, Layer, LayerTools, py, red, spv};
use rayon::prelude::*;
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The table's first line: the host refuses a table of another ABI
pub const TABLE_ABI: &str = "# GL shader ABI 2";
const VEC_TYPES: [&str; 16] = [
    "float", "vec2", "vec3", "vec4", "int", "ivec2", "ivec3", "ivec4", "uint", "uvec2", "uvec3",
    "uvec4", "bool", "mat2", "mat3", "mat4",
];
const COMMENT: &str = "# GL shaders (dm/gfx/tools/gl_shaders.py): key = sha256(GLSL source)[:16], Metal function gl_<key>_v / gl_<key>_f; a: vertex inputs, u: default uniform block members (offset:type:count:stride), r: argument buffer (slot:count), b: the block (id:size), n: named blocks (name=id:size), t: combined samplers (texture-id:sampler-id:type), o: fragment outputs";

/// Python's `\s` in a `str` pattern
macro_rules! s {
    () => {
        r"[\s\x1C-\x1F]"
    };
}

fn rx(p: &str) -> Regex {
    Regex::new(p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn brx(p: &str) -> regex::bytes::Regex {
    regex::bytes::Regex::new(p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

macro_rules! lazy {
    ($name:ident, $p:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| rx($p));
    };
}

static GL_ENTRY: LazyLock<regex::bytes::Regex> = LazyLock::new(|| brx(r"(?-u)\bvoid\s+main\s*\("));
static GL_COMMENTS: LazyLock<regex::bytes::Regex> =
    LazyLock::new(|| brx(r"(?s-u)/\*.*?\*/|//[^\r\n]*"));
static GL_MAIN_DEF: LazyLock<regex::bytes::Regex> =
    LazyLock::new(|| brx(r"(?-u)\bvoid\s+main\s*\(\s*(?:void\s*)?\)\s*\{"));
static CONTROL: LazyLock<regex::bytes::Regex> =
    LazyLock::new(|| brx(r"(?-u)[\x00-\x08\x0e-\x1f\x7f]"));

lazy!(
    BUILTIN_DECL,
    concat!(
        r"(?m)^[ \t]*(?:invariant",
        s!(),
        r"+)?(?:in|out)",
        s!(),
        r"+(?:highp",
        s!(),
        r"+|mediump",
        s!(),
        r"+|lowp",
        s!(),
        r"+)?(?:vec4|float)",
        s!(),
        r"+gl_(?:Position|FragCoord|FragDepth|PointSize)",
        s!(),
        r"*;"
    )
);
lazy!(TEXTURE_OFFSET, concat!(r"\btextureOffset", s!(), r"*\("));
lazy!(VECN, r"\b[iu]?vec[234]\b");
lazy!(IDENT_START, r"[A-Za-z_]\w*");
lazy!(WORD, r"\b\w+\b");
lazy!(
    PRECISION,
    r"(?m)^[ \t]*precision[ \t]+\w+[ \t]+\w+[ \t]*;\r?[ \t]*\r?\n"
);
lazy!(QUALIFIER, r"\b(?:lowp|mediump|highp)\b[ \t]*");
lazy!(ATTRIBUTE, r"\battribute\b");
lazy!(VARYING, r"\bvarying\b");
lazy!(VERSION, r"(?m)^[ \t]*#version[ \t]+(\d+)[^\n]*\n?");
lazy!(
    SAMPLER_DECL,
    concat!(
        r"(?:\blayout",
        s!(),
        r"*\(([^)]*)\)",
        s!(),
        r"*uniform\b|\buniform",
        s!(),
        r"+layout",
        s!(),
        r"*\(([^)]*)\))",
        s!(),
        r"+(?:(?:lowp|mediump|highp)",
        s!(),
        r"+)?[iu]?sampler\w+",
        s!(),
        r"+([^;]+);"
    )
);
lazy!(
    BINDING,
    concat!(
        r"\bbinding",
        s!(),
        r"*=",
        s!(),
        r"*(0[xX][0-9a-fA-F]+|\d+)\b"
    )
);
lazy!(DECL_NAME, concat!(r"^", s!(), r"*([A-Za-z_]\w*)"));
lazy!(
    BOOL_UNIFORM,
    concat!(
        r"\buniform",
        s!(),
        r"+(?:(?:lowp|mediump|highp)",
        s!(),
        r"+)?bool",
        s!(),
        r"+(\w+)",
        s!(),
        r"*;"
    )
);
lazy!(
    USER_LOCN,
    concat!(r"(\b(\w+)", s!(), r"+\[\[)user\(locn\d+\)")
);
lazy!(PRECISE, r"\bfast::(min|max|clamp)\(");
lazy!(ARG_ID, concat!(r"\b(\w+)", s!(), r"+\[\[id\((\d+)\)\]\]"));
lazy!(
    DEFAULT_BLOCK,
    r"(?m)^gl_DefaultUniformBlock: [^\n]*\bindex (\d+),"
);
lazy!(
    UNIFORM_ROW,
    r"(?m)^([^\s\x1C-\x1F]+): [^\n]*\bindex (-?\d+),"
);

static OLD_TEX: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        ("texture2DLod", "textureLod("),
        ("textureCubeLod", "textureLod("),
        ("texture3DLod", "textureLod("),
        ("texture2DProj", "textureProj("),
        ("texture3DProj", "textureProj("),
        ("texture2D", "texture("),
        ("texture3D", "texture("),
        ("textureCube", "texture("),
    ]
    .into_iter()
    .map(|(a, b)| (rx(&format!(r"\b{a}{}*\(", s!())), b))
    .collect()
});

fn fatal(msg: impl std::fmt::Display) -> Fatal {
    Fatal(msg.to_string())
}

/// `text_of(b)`: the bytes up to the first NUL
pub fn text_of(b: &[u8]) -> &[u8] {
    match b.iter().position(|&c| c == 0) {
        Some(z) => &b[..z],
        None => b,
    }
}

fn hex16(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .take(8)
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// `s[:n]` on characters
fn chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `str.split()`
fn split_ws(s: &str) -> Vec<&str> {
    s.split(py::isspace).filter(|w| !w.is_empty()).collect()
}

// ---- sources ------------------------------------------------------------------------------------------------------------------------------

/// `nso_strings(path)`: the `#version <digit>` strings of the module's read-only segment that start the segment or follow a NUL and define `main`
pub fn nso_strings(data: &[u8], path: &str) -> Result<Vec<Vec<u8>>, Fatal> {
    let m = ibuilder_nso::load(data, path).map_err(|e| Fatal(format!("[RED] {}", e.0)))?;
    let (mo, sz) = m.segments[1];
    let img = m
        .image
        .get(mo as usize..mo as usize + sz as usize)
        .unwrap_or(&[]);
    let mut out = Vec::new();
    let pat = b"#version ";
    let mut p = 0usize;
    while p + pat.len() < img.len() {
        if (p == 0 || img[p - 1] == 0)
            && img[p..].starts_with(pat)
            && img[p + pat.len()].is_ascii_digit()
        {
            let end = (p + 0x10000).min(img.len());
            let t = text_of(&img[p..end]);
            if GL_ENTRY.is_match(t) {
                out.push(t.to_vec());
            }
            p += pat.len() + 1;
        } else {
            p += 1;
        }
    }
    Ok(out)
}

/// `is_standalone_glsl(source)`: text GLSL that defines `main()` (not a snippet, not binary)
pub fn is_standalone_glsl(source: &[u8]) -> bool {
    !source.is_empty()
        && !CONTROL.is_match(source)
        && GL_MAIN_DEF.is_match(&GL_COMMENTS.replace_all(source, &b" "[..]))
}

/// `os.path.splitext(name)[1]`
fn splitext(name: &str) -> &str {
    let stem = name.trim_start_matches('.');
    match stem.rfind('.') {
        Some(i) => &stem[i..],
        None => "",
    }
}

fn pair_stage(name: &str) -> Option<&'static str> {
    match splitext(name).to_lowercase().as_str() {
        ".vs" => Some("vert"),
        ".fs" => Some("frag"),
        _ => None,
    }
}

fn io_err(what: &str, e: std::io::Error) -> Fatal {
    Fatal(format!("[RED] {what}: {e}"))
}

/// `paired_sources(folder, names)`: the standalone vertex and fragment sources of one folder, when it holds both
fn paired_sources(files: &[walk::File]) -> Result<HashMap<String, (Vec<u8>, &'static str)>, Fatal> {
    let cand: Vec<(&walk::File, &'static str)> = files
        .iter()
        .filter_map(|f| pair_stage(&f.name).map(|s| (f, s)))
        .collect();
    let kinds: HashSet<&str> = cand.iter().map(|c| c.1).collect();
    if kinds.len() != 2 {
        return Ok(HashMap::new());
    }
    let mut found = HashMap::new();
    for (f, st) in cand {
        let b = f.read().map_err(|e| io_err(&f.rel, e))?;
        let src = text_of(&b);
        if is_standalone_glsl(src) {
            found.insert(f.name.clone(), (src.to_vec(), st));
        }
    }
    let kinds: HashSet<&str> = found.values().map(|v| v.1).collect();
    Ok(if kinds.len() == 2 {
        found
    } else {
        HashMap::new()
    })
}

/// One shader source: its bytes, the stage its name says (`None`: try both), where it came from
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub src: Vec<u8>,
    pub stage: Option<&'static str>,
    pub at: String,
}

/// `sources(roots)`
pub fn sources(roots: &[PathBuf]) -> Result<Vec<Item>, Fatal> {
    let mut out = Vec::new();
    for r in roots {
        match walk::root(r)? {
            Root::Missing => {}
            Root::File(p) => {
                let d = std::fs::read(&p).map_err(|e| io_err(&p.display().to_string(), e))?;
                if d.get(..4) == Some(b"NSO0") {
                    let base = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    for (k, s) in nso_strings(&d, &p.display().to_string())?
                        .into_iter()
                        .enumerate()
                    {
                        out.push(Item {
                            src: s,
                            stage: None,
                            at: format!("{base}#{k}"),
                        });
                    }
                }
            }
            Root::Tree(dirs) => {
                for d in &dirs {
                    let paired = paired_sources(&d.files)?;
                    for f in &d.files {
                        let low = f.name.to_lowercase();
                        if low.ends_with(".mvgl") {
                            let src = f.loc.open().map_err(|e| io_err(&f.rel, e))?;
                            let a = match Archive::open(Box::new(src), &f.rel) {
                                Ok(a) => a,
                                Err(e) if e.value => continue, // the sound archives are CRI AFS2 under the same extension
                                Err(e) => return Err(Fatal(format!("[RED] {}", e.msg))),
                            };
                            for nm in a.files.keys() {
                                if !nm.ends_with(".shad") {
                                    continue;
                                }
                                let st = if nm.ends_with("_vp.shad") {
                                    Some("vert")
                                } else if nm.ends_with("_fp.shad") {
                                    Some("frag")
                                } else {
                                    None
                                };
                                let b = a
                                    .read(nm)
                                    .map_err(|e| Fatal(format!("[RED] {}: {}", f.rel, e.msg)))?;
                                out.push(Item {
                                    src: text_of(&b).to_vec(),
                                    stage: st,
                                    at: format!("/{}:{nm}", f.rel),
                                });
                            }
                        } else if low.ends_with(".vert")
                            || low.ends_with(".frag")
                            || low.ends_with(".glsl")
                        {
                            let b = f.read().map_err(|e| io_err(&f.rel, e))?;
                            let src = text_of(&b);
                            let ext = low.rsplit_once('.').map(|x| x.1).unwrap_or("");
                            if ext == "glsl" && !is_standalone_glsl(src) {
                                continue;
                            }
                            out.push(Item {
                                src: src.to_vec(),
                                stage: match ext {
                                    "vert" => Some("vert"),
                                    "frag" => Some("frag"),
                                    _ => None,
                                },
                                at: format!("/{}", f.rel),
                            });
                        } else if let Some((src, st)) = paired.get(&f.name) {
                            out.push(Item {
                                src: src.clone(),
                                stage: Some(st),
                                at: format!("/{}", f.rel),
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

// ---- the source rewrites -------------------------------------------------------------------------------------------------------------------

fn sub(re: &Regex, t: &str, rep: &str) -> String {
    re.replace_all(t, regex::NoExpand(rep)).into_owned()
}

/// `call_args(t, i)`: the arguments of the call whose `(` is at byte `i`, and the index after its `)`; `None` is the Python's `ValueError('unbalanced parentheses')`
pub fn call_args(t: &str, i: usize) -> Option<(Vec<&str>, usize)> {
    let b = t.as_bytes();
    let (mut depth, mut args, mut start) = (0i64, Vec::new(), i + 1);
    for k in i..b.len() {
        match b[k] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    args.push(&t[start..k]);
                    return Some((args, k + 1));
                }
            }
            b',' if depth == 1 => {
                args.push(&t[start..k]);
                start = k + 1;
            }
            _ => {}
        }
    }
    None
}

fn skip_space(t: &str) -> &str {
    t.trim_start_matches(py::isspace)
}

/// `own_name(t, decl, name, new, out)`
fn own_name(t: &str, decl: &Regex, name: &str, new: &str, out: &str) -> String {
    if !decl.is_match(t) {
        return t.to_string();
    }
    let t = sub(decl, t, out);
    sub(&rx(&format!(r"\b{}\b", regex::escape(name))), &t, new)
}

/// `function_semicolons(t)`: no `;` right after the `}` that closes a function body at file scope
pub fn function_semicolons(t: &str) -> String {
    let b = t.as_bytes();
    let mut out = String::with_capacity(t.len());
    let (mut depth, mut body, mut i, mut k) = (0i64, false, 0usize, 0usize);
    while k < b.len() {
        let c = b[k];
        if c == b'{' {
            if depth == 0 {
                body = py::rstrip(&t[..k]).ends_with(')');
            }
            depth += 1;
        } else if c == b'}' && depth != 0 {
            depth -= 1;
            if depth == 0 && body {
                let rest = &t[k + 1..];
                let after = skip_space(rest);
                if after.starts_with(';') {
                    out.push_str(&t[i..k + 1]);
                    i = k + 1 + (rest.len() - after.len()) + 1;
                    k = i;
                    continue;
                }
            }
        }
        k += 1;
    }
    out.push_str(&t[i..]);
    out
}

/// `rename_identifier(t, name, base, renamed, keep_calls)`: the identifier renamed to a free name (a call to it kept with `keep_calls`)
pub fn rename_identifier(
    t: &str,
    name: &str,
    base: &str,
    renamed: &mut HashMap<String, String>,
    keep_calls: bool,
) -> String {
    let re = rx(&format!(r"\b{}\b", regex::escape(name)));
    let hits: Vec<(usize, usize)> = re
        .find_iter(t)
        .filter(|m| !keep_calls || !skip_space(&t[m.end()..]).starts_with('('))
        .map(|m| (m.start(), m.end()))
        .collect();
    if hits.is_empty() {
        return t.to_string();
    }
    let used: HashSet<&str> = WORD.find_iter(t).map(|m| m.as_str()).collect();
    let mut new = base.to_string();
    let mut i = 0;
    while used.contains(new.as_str()) {
        i += 1;
        new = format!("{base}_{i}");
    }
    renamed.insert(new.clone(), name.to_string());
    let mut out = String::with_capacity(t.len());
    let mut at = 0;
    for (a, b) in hits {
        out.push_str(&t[at..a]);
        out.push_str(&new);
        at = b;
    }
    out.push_str(&t[at..]);
    out
}

/// `fixups(t, stage, renamed)`: on the preprocessed text, what NVIDIA's compiler takes and glslang does not. `Err` is the Python's uncaught `ValueError`.
pub fn fixups(
    t: &str,
    stage: &str,
    renamed: &mut HashMap<String, String>,
) -> Result<String, Fatal> {
    static FRONT: LazyLock<Regex> = LazyLock::new(|| {
        rx(concat!(
            r"(?m)^[ \t]*out",
            s!(),
            r"+vec4",
            s!(),
            r"+gl_FrontColor",
            s!(),
            r"*;"
        ))
    });
    static COLOR: LazyLock<Regex> = LazyLock::new(|| {
        rx(concat!(
            r"(?m)^[ \t]*in",
            s!(),
            r"+vec4",
            s!(),
            r"+gl_Color",
            s!(),
            r"*;"
        ))
    });
    static FRAG: LazyLock<Regex> = LazyLock::new(|| {
        rx(concat!(
            r"(?m)^[ \t]*out",
            s!(),
            r"+vec4",
            s!(),
            r"+gl_FragColor",
            s!(),
            r"*;"
        ))
    });
    let mut t = function_semicolons(t);
    for (a, b) in OLD_TEX.iter() {
        t = sub(a, &t, b);
    }
    t = sub(&BUILTIN_DECL, &t, "");
    t = rename_identifier(&t, "sampler", "dm_sampler", renamed, false);
    if stage == "vert" {
        t = own_name(
            &t,
            &FRONT,
            "gl_FrontColor",
            "dm_Color",
            "out vec4 dm_Color;",
        );
    } else {
        t = own_name(&t, &COLOR, "gl_Color", "dm_Color", "in vec4 dm_Color;");
        t = own_name(
            &t,
            &FRAG,
            "gl_FragColor",
            "dm_FragColor",
            "out vec4 dm_FragColor;",
        );
    }
    let mut out = String::with_capacity(t.len());
    let mut i = 0usize;
    for m in TEXTURE_OFFSET.find_iter(&t) {
        if m.start() < i {
            continue;
        }
        let (args, end) = call_args(&t, m.end() - 1)
            .ok_or_else(|| fatal("ValueError: unbalanced parentheses"))?;
        if args.len() == 3 && IDENT_START.is_match(&VECN.replace_all(args[2], "")) {
            let (s_, p_, o_) = (py::strip(args[0]), py::strip(args[1]), py::strip(args[2]));
            out.push_str(&t[i..m.start()]);
            out.push_str(&format!(
                "texture({s_}, ({p_}) + vec2({o_}) / vec2(textureSize({s_}, 0)))"
            ));
            i = end;
        }
    }
    out.push_str(&t[i..]);
    Ok(out)
}

/// `promote_legacy(t, stage, renamed, desktop)`: GLSL ES 1.00 to `#version 310 es`, legacy desktop GLSL to `#version 450 core`
pub fn promote_legacy(
    t: &str,
    stage: &str,
    renamed: &mut HashMap<String, String>,
    desktop: bool,
) -> String {
    let mut t = sub(&PRECISION, t, "");
    t = sub(&QUALIFIER, &t, "");
    t = sub(&ATTRIBUTE, &t, "in");
    t = sub(&VARYING, &t, if stage == "vert" { "out" } else { "in" });
    if stage == "frag" && t.contains("gl_FragColor") {
        t = format!(
            "out vec4 gm_FragColor;\n{}",
            t.replace("gl_FragColor", "gm_FragColor")
        );
    }
    t = rename_identifier(&t, "sample", "gm_sample", renamed, false);
    t = rename_identifier(&t, "texture", "dm_texture", renamed, true);
    let header = if desktop {
        "#version 450 core\n"
    } else {
        "#version 310 es\nprecision highp float;\nprecision highp int;\n"
    };
    format!("{header}{t}")
}

/// `es100(t, stage, renamed)`
pub fn es100(t: &str, stage: &str, renamed: &mut HashMap<String, String>) -> String {
    promote_legacy(t, stage, renamed, false)
}

/// `legacy_desktop_io(t, stage)`: the compatibility varyings of GLSL 1.10 / 1.20 / 1.30 as plain ones
pub fn legacy_desktop_io(t: &str, stage: &str) -> String {
    let vert = stage == "vert";
    let direction = if vert { "out" } else { "in" };
    let color = if vert { "gl_FrontColor" } else { "gl_Color" };
    let secondary = if vert {
        "gl_FrontSecondaryColor"
    } else {
        "gl_SecondaryColor"
    };
    let mut t = t.to_string();
    let mut decl: Vec<String> = Vec::new();
    for (old, name, ty) in [
        (color, "dm_LegacyColor", "vec4"),
        (secondary, "dm_LegacySecondaryColor", "vec4"),
        ("gl_FogFragCoord", "dm_LegacyFog", "float"),
    ] {
        let re = rx(&format!(r"\b{old}\b"));
        if re.is_match(&t) {
            t = sub(&re, &t, name);
            decl.push(format!("{direction} {ty} {name};"));
        }
    }
    let mut arrays = vec![(
        "gl_TexCoord",
        "dm_LegacyTexCoord",
        format!("{direction} vec4"),
    )];
    if !vert {
        arrays.push(("gl_FragData", "dm_LegacyFragData", "out vec4".to_string()));
    }
    for (old, name, qualifier) in arrays {
        let re = rx(&format!(
            r"\b{old}{s}*\[{s}*(0|[1-9][0-9]*){s}*\]",
            s = s!()
        ));
        let mut idx: Vec<String> = re
            .captures_iter(&t)
            .map(|c| c[1].to_string())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        idx.sort_by(|a, b| (a.len(), a).cmp(&(b.len(), b)));
        for i in &idx {
            let layout = if old == "gl_FragData" {
                format!("layout(location = {i}) ")
            } else {
                String::new()
            };
            decl.push(format!("{layout}{qualifier} {name}{i};"));
        }
        t = re
            .replace_all(&t, |c: &regex::Captures| format!("{name}{}", &c[1]))
            .into_owned();
    }
    if decl.is_empty() {
        return t;
    }
    let lines = py::splitlines(&t);
    let mut at = 1;
    while at < lines.len()
        && (py::strip(lines[at]).is_empty()
            || py::lstrip(lines[at]).starts_with("#extension")
            || py::lstrip(lines[at]).starts_with("#line"))
    {
        at += 1;
    }
    let at = at.min(lines.len());
    let mut all: Vec<&str> = lines[..at].to_vec();
    all.extend(decl.iter().map(|s| s.as_str()));
    all.extend_from_slice(&lines[at..]);
    let mut s = all.join("\n");
    if t.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// `int(s, 0)` of `0[xX]<hex>` or decimal digits: `None` where the Python raises (a decimal with a leading zero that is not all zeros)
fn int0(s: &str) -> Option<u128> {
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u128::from_str_radix(h, 16).ok();
    }
    if s.len() > 1 && s.starts_with('0') && !s.bytes().all(|c| c == b'0') {
        return None;
    }
    s.parse::<u128>().ok()
}

/// `explicit_sampler_bindings(text)`: the texture units explicit `layout(binding = N)` sampler declarations give. `Err`: the Python's `ValueError` of `int(.., 0)`.
pub fn explicit_sampler_bindings(text: &str) -> Result<HashMap<String, u128>, Fatal> {
    let mut out = HashMap::new();
    for m in SAMPLER_DECL.captures_iter(text) {
        let qual = m.get(1).or(m.get(2)).map(|g| g.as_str()).unwrap_or("");
        let Some(b) = BINDING.captures(qual) else {
            continue;
        };
        let unit = int0(&b[1]).ok_or_else(|| {
            fatal(format!(
                "ValueError: invalid literal for int() with base 0: '{}'",
                &b[1]
            ))
        })?;
        for decl in m[3].split(',') {
            if let Some(nm) = DECL_NAME.captures(decl) {
                out.insert(nm[1].to_string(), unit);
            }
        }
    }
    Ok(out)
}

/// `active_default_uniforms(log)`: the active members of `gl_DefaultUniformBlock` in glslang's `-q` reflection; `None` when the log has no reflection
pub fn active_default_uniforms(log: &str) -> Option<HashSet<String>> {
    let (_, data) = log.split_once("Uniform reflection:\n")?;
    let (uniforms, blocks) = data.split_once("\nUniform block reflection:\n")?;
    let first = blocks.split_once("\n\n").map(|p| p.0).unwrap_or(blocks);
    let Some(block) = DEFAULT_BLOCK.captures(first) else {
        return Some(HashSet::new());
    };
    let idx = &block[1];
    Some(
        UNIFORM_ROW
            .captures_iter(uniforms)
            .filter(|c| &c[2] == idx)
            .map(|c| c[1].to_string())
            .collect(),
    )
}

/// `game_names(f, renamed)`: the renamed names in the name part of each field given back
pub fn game_names(f: &[String], renamed: &HashMap<String, String>) -> Vec<String> {
    f.iter()
        .map(|x| {
            let (k, sep, rest) = match x.find('=') {
                Some(i) => (&x[..i], "=", &x[i + 1..]),
                None => (x.as_str(), "", ""),
            };
            let k = WORD.replace_all(k, |c: &regex::Captures| {
                renamed
                    .get(&c[0])
                    .cloned()
                    .unwrap_or_else(|| c[0].to_string())
            });
            format!("{k}{sep}{rest}")
        })
        .collect()
}

// ---- SPIR-V and reflection ----------------------------------------------------------------------------------------------------------------

/// `argument_names(spv)`: the bound variables of the module renamed `dm_gl_resource_<id>` (descriptor sets to 0); returns the file's new bytes and
/// (sampler name -> new name, `_<block type id>` -> new name)
#[allow(clippy::type_complexity)]
pub fn argument_names(
    data: &[u8],
) -> Result<(Vec<u8>, HashMap<String, String>, HashMap<String, String>), Fatal> {
    if !data.len().is_multiple_of(4) {
        return Err(fatal(format!(
            "struct.error: unpack requires a buffer of {} bytes",
            data.len() / 4 * 4
        )));
    }
    let words = spv::words(data);
    let idx = |w: &[u32], k: usize| -> Result<u32, Fatal> {
        w.get(k)
            .copied()
            .ok_or_else(|| fatal("IndexError: list index out of range"))
    };
    let mut ins: Vec<Vec<u32>> = Vec::new();
    let mut names: BTreeMap<u32, String> = BTreeMap::new();
    let mut bound: Vec<u32> = Vec::new();
    let mut pointers: HashMap<u32, u32> = HashMap::new();
    let mut variables: HashMap<u32, (u32, u32)> = HashMap::new();
    let mut i = 5usize;
    while i < words.len() {
        let (n, op) = ((words[i] >> 16) as usize, words[i] & 0xffff);
        if n == 0 {
            return Err(fatal("ValueError: SPIR-V instruction length 0"));
        }
        let w = words[i..(i + n).min(words.len())].to_vec();
        match op {
            5 => {
                let id = idx(&w, 1)?;
                let s = String::from_utf8(spv::literal(w.get(2..).unwrap_or(&[])))
                    .map_err(|_| fatal("UnicodeDecodeError in an OpName"))?;
                names.insert(id, s);
            }
            71 if idx(&w, 2)? == 33 => {
                let v = idx(&w, 1)?;
                if !bound.contains(&v) {
                    bound.push(v);
                }
            }
            32 => {
                pointers.insert(idx(&w, 1)?, idx(&w, 3)?);
            }
            59 => {
                variables.insert(idx(&w, 2)?, (idx(&w, 1)?, idx(&w, 3)?));
            }
            _ => {}
        }
        ins.push(w);
        i += n;
    }
    bound.sort_unstable();
    let mut prefix = "dm_gl_resource_".to_string();
    while names.values().any(|s| s.contains(&prefix)) {
        prefix.push('x');
    }
    let renamed: BTreeMap<u32, String> =
        bound.iter().map(|&v| (v, format!("{prefix}{v}"))).collect();
    let mut new_names = Vec::new();
    for (&v, name) in &renamed {
        let sw = spv::string_words(name.as_bytes());
        new_names.push((((2 + sw.len()) as u32) << 16) | 5);
        new_names.push(v);
        new_names.extend(sw);
    }
    let mut out: Vec<u32> = words[..5.min(words.len())].to_vec();
    let mut added = false;
    for mut w in ins {
        let op = w[0] & 0xffff;
        if !added && matches!(op, 5 | 6 | 71 | 72) {
            out.extend_from_slice(&new_names);
            added = true;
        }
        if op == 5 && renamed.contains_key(&w[1]) {
            continue;
        }
        if op == 71 && renamed.contains_key(&w[1]) && w[2] == 34 {
            if w.len() < 4 {
                return Err(fatal("IndexError: list assignment index out of range"));
            }
            w[3] = 0;
        }
        out.extend(w);
    }
    let var = |v: &u32| -> Result<(u32, u32), Fatal> {
        variables
            .get(v)
            .copied()
            .ok_or_else(|| fatal(format!("KeyError: {v}")))
    };
    let mut textures = HashMap::new();
    let mut buffers = HashMap::new();
    for (v, name) in &renamed {
        let (ty, storage) = var(v)?;
        if storage == 0 {
            textures.insert(names.get(v).cloned().unwrap_or_default(), name.clone());
        } else if storage == 2 {
            let p = pointers
                .get(&ty)
                .ok_or_else(|| fatal(format!("KeyError: {ty}")))?;
            buffers.insert(format!("_{p}"), name.clone());
        }
    }
    Ok((spv::bytes(&out), textures, buffers))
}

fn key_err(k: &str) -> Fatal {
    fatal(format!("KeyError: '{k}'"))
}

fn get<'a>(v: &'a Value, k: &str) -> Result<&'a Value, Fatal> {
    v.get(k).ok_or_else(|| key_err(k))
}

/// `str(x)` of a JSON string or number
fn s_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        other => other.to_string(),
    }
}

fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(|x| x.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

/// truthiness of `refl.get(k)`
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
    }
}

fn glsl_type(t: &str, refl: &Value) -> Result<String, Fatal> {
    match refl.get("types").and_then(|x| x.get(t)) {
        Some(ty) => Ok(s_of(get(ty, "name")?)),
        None => Ok(t.to_string()),
    }
}

fn as_int(v: &Value) -> Result<i128, Fatal> {
    v.as_i64()
        .map(|x| x as i128)
        .or(v.as_u64().map(|x| x as i128))
        .ok_or_else(|| fatal("TypeError: a JSON value that is not an integer"))
}

/// `uni_members(refl, ms, prefix, base, f, bools, active)`: `Ok(Some(error))` is the Python's returned error string
#[allow(clippy::too_many_arguments)]
fn uni_members(
    refl: &Value,
    ms: &[Value],
    prefix: &str,
    base: i128,
    f: &mut Vec<String>,
    bools: &HashSet<String>,
    active: &HashSet<String>,
) -> Result<Option<String>, Fatal> {
    let types = refl.get("types");
    for mb in ms {
        let arr = list(mb, "array");
        let mname = s_of(get(mb, "name")?);
        let name = format!("{prefix}{mname}");
        if arr.len() > 1 {
            return Ok(Some(format!("uniform {name}: arrays of arrays")));
        }
        let mty = s_of(get(mb, "type")?);
        let sub_ty = types.and_then(|t| t.get(&mty));
        if sub_ty.and_then(|t| t.get("members")).is_some() {
            if !arr.is_empty() {
                return Ok(Some(format!(
                    "uniform {name}: an array of structs is not implemented"
                )));
            }
            let members = list(sub_ty.unwrap(), "members");
            let off = as_int(get(mb, "offset")?)?;
            if let Some(e) = uni_members(
                refl,
                members,
                &format!("{name}."),
                base + off,
                f,
                bools,
                active,
            )? {
                return Ok(Some(e));
            }
            continue;
        }
        if !active.contains(&name) {
            continue;
        }
        let mut ty = glsl_type(&mty, refl)?;
        if ty == "uint" && bools.contains(&mname) {
            ty = "bool".to_string();
        }
        if !VEC_TYPES.contains(&ty.as_str()) {
            return Ok(Some(format!(
                "uniform {name}: type {ty} (structs are not implemented)"
            )));
        }
        let off = as_int(get(mb, "offset")?)?;
        let count = arr.first().map(s_of).unwrap_or_else(|| "0".to_string());
        let stride = mb
            .get("array_stride")
            .map(s_of)
            .unwrap_or_else(|| "0".to_string());
        f.push(format!("u:{name}={}:{ty}:{count}:{stride}", base + off));
    }
    Ok(None)
}

// ---- one shader -----------------------------------------------------------------------------------------------------------------------------

fn err_lines(r: &crate::ToolRun) -> String {
    let all = format!("{}{}", r.stdout, r.stderr);
    let l: Vec<&str> = py::splitlines(&all)
        .into_iter()
        .filter(|l| l.contains("ERROR") && !l.contains("compilation"))
        .collect();
    chars(&l.join(" | "), 400)
}

fn wr(p: &Path, b: &[u8]) -> Result<(), Fatal> {
    std::fs::write(p, b).map_err(|e| io_err(&p.display().to_string(), e))
}

/// A translated stage: the MSL file's name and text, and the table fields
pub struct Translated {
    pub msl_name: String,
    pub msl: String,
    pub fields: Vec<String>,
}

/// `translate(src, stage, key, tmp)`: `Ok(Err(why))` is the Python's returned error string, `Err` an exception it does not catch
pub fn translate(
    src: &[u8],
    stage: &str,
    key: &str,
    tmp: &Path,
    tools: &dyn LayerTools,
) -> Result<Result<Translated, String>, Fatal> {
    let t = py::latin1(src);
    let mut renamed: HashMap<String, String> = HashMap::new();
    let v = VERSION.captures(&t);
    let vnum = v.as_ref().map(|c| c[1].to_string());
    let desktop_legacy = matches!(vnum.as_deref(), Some("110" | "120" | "130"));
    let mut src = src.to_vec();
    if v.is_none() || vnum.as_deref() == Some("100") || desktop_legacy {
        let body = match &v {
            Some(c) => t.replacen(&c[0], "", 1),
            None => t.clone(),
        };
        let p = promote_legacy(&body, stage, &mut renamed, desktop_legacy);
        src = py::to_latin1(&p).ok_or_else(|| fatal("UnicodeEncodeError: 'latin-1'"))?;
    }
    let g = tmp.join(format!("{key}.{stage}"));
    let gs = g.display().to_string();
    let spv_path = format!("{gs}.spv");
    let msl_name = format!("{key}_{}.metal", &stage[..1]);
    let msl_path = tmp.join(&msl_name);
    let msl_s = msl_path.display().to_string();
    let gin = format!("{gs}.in");
    wr(Path::new(&gin), &src)?;
    let r = run(tools, Tool::Glslang, &["-E", "-S", stage, &gin]);
    if r.rc != 0 {
        return Ok(Err(format!("glslang -E: {}", err_lines(&r))));
    }
    let pre: String = match r.stdout.find("#version") {
        Some(i) => r.stdout[i..].to_string(),
        None => r
            .stdout
            .chars()
            .last()
            .map(|c| c.to_string())
            .unwrap_or_default(),
    };
    let pre = if desktop_legacy {
        legacy_desktop_io(&pre, stage)
    } else {
        pre
    };
    let fixed = fixups(&pre, stage, &mut renamed)?;
    wr(&g, fixed.as_bytes())?;
    let r = run(
        tools,
        Tool::Glslang,
        &[
            "-V", "-R", "--aml", "--amb", "-q", "-S", stage, &gs, "-o", &spv_path,
        ],
    );
    if r.rc != 0 {
        return Ok(Err(format!("glslang: {}", err_lines(&r))));
    }
    let Some(active) = active_default_uniforms(&r.stdout) else {
        return Ok(Err(
            "glslang: could not read active uniform reflection".to_string()
        ));
    };
    let rr = run(tools, Tool::SpirvCross, &[&spv_path, "--reflect"]);
    if rr.rc != 0 {
        return Err(fatal(format!(
            "CalledProcessError: spirv-cross --reflect returned {}: {}",
            rr.rc,
            rr.stderr.trim()
        )));
    }
    let refl: Value =
        serde_json::from_str(&rr.stdout).map_err(|e| fatal(format!("JSONDecodeError: {e}")))?;
    for k in [
        "separate_images",
        "separate_samplers",
        "images",
        "ssbos",
        "push_constants",
    ] {
        if truthy(refl.get(k)) {
            return Ok(Err(format!("{k} are not implemented")));
        }
    }
    for k in ["textures", "ubos"] {
        for res in list(&refl, k) {
            if truthy(res.get("array")) {
                return Ok(Err(format!(
                    "{} array {}",
                    if k == "textures" {
                        "sampler"
                    } else {
                        "uniform block"
                    },
                    s_of(get(res, "name")?)
                )));
            }
        }
    }
    let data = std::fs::read(&spv_path).map_err(|e| io_err(&spv_path, e))?;
    let (new_spv, textures, buffers) = argument_names(&data)?;
    wr(Path::new(&spv_path), &new_spv)?;
    let ep = format!("gl_{key}_{}", &stage[..1]);
    let mut argv: Vec<&str> = vec![
        &spv_path,
        "--msl",
        "--msl-version",
        "20300",
        "--msl-argument-buffers",
        "--msl-argument-buffer-tier",
        "1",
        "--msl-texture-buffer-native",
        "--rename-entry-point",
        "main",
        &ep,
        stage,
        "--output",
        &msl_s,
    ];
    if stage == "vert" {
        argv.push("--fixup-clipspace");
    }
    let r = run(tools, Tool::SpirvCross, &argv);
    if r.rc != 0 {
        return Ok(Err(format!(
            "spirv-cross: {}",
            chars(py::strip(&r.stderr), 300)
        )));
    }
    let m = std::fs::read(&msl_path).map_err(|e| io_err(&msl_s, e))?;
    let m = py::text_mode(&String::from_utf8_lossy(&m));
    let m = USER_LOCN
        .replace_all(&m, |c: &regex::Captures| {
            let n = renamed.get(&c[2]).map(|s| s.as_str()).unwrap_or(&c[2]);
            format!("{}user({n})", &c[1])
        })
        .into_owned();
    let m = PRECISE.replace_all(&m, "precise::${1}(").into_owned();
    wr(&msl_path, m.as_bytes())?;
    let mut ids: HashMap<String, u64> = HashMap::new();
    for c in ARG_ID.captures_iter(&m) {
        ids.insert(c[1].to_string(), c[2].parse().unwrap_or(u64::MAX));
    }
    let mut f = vec![format!(
        "r:{}:{}",
        if ids.is_empty() { -1 } else { 0 },
        ids.values().max().map(|&x| x as i128).unwrap_or(-1) + 1
    )];
    if stage == "vert" {
        for v in list(&refl, "inputs") {
            f.push(format!(
                "a:{}={}:{}",
                s_of(get(v, "name")?),
                s_of(get(v, "location")?),
                s_of(get(v, "type")?)
            ));
        }
    } else {
        for v in list(&refl, "outputs") {
            f.push(format!(
                "o:{}={}",
                s_of(get(v, "name")?),
                s_of(get(v, "location")?)
            ));
        }
    }
    let bools: HashSet<String> = BOOL_UNIFORM
        .captures_iter(&pre)
        .map(|c| c[1].to_string())
        .collect();
    let types = refl.get("types");
    for u in list(&refl, "ubos") {
        let uty = s_of(get(u, "type")?);
        let bname = buffers.get(&uty).ok_or_else(|| key_err(&uty))?;
        let Some(&aid) = ids.get(bname) else {
            continue;
        };
        let t = get(types.ok_or_else(|| key_err("types"))?, &uty)?;
        let tname = s_of(get(t, "name")?);
        let size = s_of(get(u, "block_size")?);
        if tname != "gl_DefaultUniformBlock" {
            f.push(format!("n:{tname}={aid}:{size}"));
            continue;
        }
        f.push(format!("b:{aid}:{size}"));
        if let Some(e) = uni_members(&refl, list(t, "members"), "", 0, &mut f, &bools, &active)? {
            return Ok(Err(e));
        }
    }
    let bindings = explicit_sampler_bindings(&fixed)?;
    for t in list(&refl, "textures") {
        let tn = s_of(get(t, "name")?);
        let name = textures.get(&tn).ok_or_else(|| key_err(&tn))?;
        let Some(&id) = ids.get(name) else {
            continue;
        };
        let unit = bindings.get(&tn).copied().unwrap_or(0);
        let suffix = if unit != 0 {
            format!(":{unit}")
        } else {
            String::new()
        };
        let smp = ids
            .get(&format!("{name}Smplr"))
            .map(|&x| x as i128)
            .unwrap_or(-1);
        f.push(format!(
            "t:{tn}={id}:{smp}:{}{suffix}",
            s_of(get(t, "type")?)
        ));
    }
    Ok(Ok(Translated {
        msl_name,
        msl: m,
        fields: game_names(&f, &renamed),
    }))
}

/// A row of `one()`: a translated stage, or (stage `None`) the errors of a source no stage of which translates
pub struct Row {
    pub key: String,
    pub stage: Option<&'static str>,
    pub fields: Vec<String>,
    pub msl: Option<(String, String)>,
    pub at: String,
}

/// `one(item, tmp)`
pub fn one(item: &Item, tmp: &Path, tools: &dyn LayerTools) -> Result<Vec<Row>, Fatal> {
    let key = hex16(&item.src);
    let stages: Vec<&'static str> = match item.stage {
        Some(s) => vec![s],
        None => vec!["vert", "frag"],
    };
    let (mut out, mut errs) = (Vec::new(), Vec::new());
    for st in stages {
        match translate(&item.src, st, &key, tmp, tools)? {
            Ok(t) => out.push(Row {
                key: key.clone(),
                stage: Some(st),
                fields: t.fields,
                msl: Some((t.msl_name, t.msl)),
                at: item.at.clone(),
            }),
            Err(e) => errs.push(format!("{st}: {e}")),
        }
    }
    if out.is_empty() {
        out.push(Row {
            key,
            stage: None,
            fields: errs,
            msl: None,
            at: item.at.clone(),
        });
    }
    Ok(out)
}

/// `gl_shaders.py <roots>... <out> [--fixed-function]`. `work`: a scratch folder (the Python's `tempfile.TemporaryDirectory()`); `jobs`: threads.
pub fn build(
    roots: &[PathBuf],
    tools: &dyn LayerTools,
    work: &Path,
    jobs: usize,
    fixed: bool,
) -> Result<Layer, Fatal> {
    let mut items: Vec<Item> = Vec::new();
    let mut seen_src: HashSet<(Vec<u8>, Option<&'static str>)> = HashSet::new();
    for it in sources(roots)? {
        if seen_src.insert((it.src.clone(), it.stage)) {
            items.push(it);
        }
    }
    let names: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
    if fixed && !items.is_empty() {
        return red("GLSL sources found in a fixed-function build");
    }
    if !fixed && items.is_empty() {
        return red(format!("no GLSL shaders in {}", names.join(" ")));
    }
    std::fs::create_dir_all(work).map_err(|e| io_err(&work.display().to_string(), e))?;
    // each source in a folder of its own: two sources with the same key and stage (one named, one tried as both) never write the same file at once
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs.max(1))
        .build()
        .map_err(|e| fatal(e.to_string()))?;
    let results: Vec<Result<Vec<Row>, Fatal>> = pool.install(|| {
        items
            .par_iter()
            .enumerate()
            .map(|(i, it)| {
                let d = work.join(i.to_string());
                std::fs::create_dir_all(&d).map_err(|e| io_err(&d.display().to_string(), e))?;
                one(it, &d, tools)
            })
            .collect()
    });
    let mut rows = Vec::new();
    for r in results {
        rows.extend(r?);
    }
    // seen: (key, stage) -> index in `kept`, in the order of the rows
    let mut seen: HashMap<(String, &'static str), usize> = HashMap::new();
    let mut kept: Vec<Row> = Vec::new();
    let mut failed: Vec<Row> = Vec::new();
    for row in rows {
        match row.stage {
            None => failed.push(row),
            Some(st) => {
                if let std::collections::hash_map::Entry::Vacant(e) =
                    seen.entry((row.key.clone(), st))
                {
                    e.insert(kept.len());
                    kept.push(row);
                }
            }
        }
    }
    let mut log = Vec::new();
    let keys: HashSet<&str> = kept.iter().map(|r| r.key.as_str()).collect();
    for f in &failed {
        if !keys.contains(f.key.as_str()) {
            log.push(format!(
                "[RED] {} ({}) does not translate: {}",
                f.at,
                f.key,
                f.fields.join(" / ")
            ));
        }
    }
    if !fixed && kept.is_empty() {
        return red("no GLSL shaders translated");
    }
    let mut tsv = vec![TABLE_ABI.to_string(), COMMENT.to_string()];
    if fixed {
        tsv.push("# Fixed-function OpenGL uses the host Metal shaders".to_string());
    } else {
        let g = run(tools, Tool::Glslang, &["--version"]);
        let s = run(tools, Tool::SpirvCross, &["--revision"]);
        let gv: Vec<&str> = split_ws(&g.stdout).into_iter().take(3).collect();
        let both = format!("{}{}", s.stdout, s.stderr);
        tsv.push(format!(
            "# {}; spirv-cross {}",
            gv.join(" "),
            split_ws(&both).join(" ")
        ));
    }
    let mut sorted: Vec<&Row> = kept.iter().collect();
    sorted.sort_by(|a, b| (&a.key, a.stage).cmp(&(&b.key, b.stage)));
    for r in sorted {
        tsv.push(format!(
            "{}\t{}\t{}\t{}",
            r.key,
            r.stage.unwrap(),
            r.fields.join(" "),
            r.at
        ));
    }
    let mut msl: Vec<(String, String)> = kept.iter().filter_map(|r| r.msl.clone()).collect();
    if fixed {
        msl.push((
            "empty.metal".to_string(),
            "#include <metal_stdlib>\n".to_string(),
        ));
    }
    let vert = kept.iter().filter(|r| r.stage == Some("vert")).count();
    log.push(format!(
        "{} shaders ({vert} vertex){}",
        kept.len(),
        if failed.is_empty() {
            String::new()
        } else {
            format!("; {} failed", failed.len())
        }
    ));
    Ok(Layer {
        api: Api::Gl,
        table: tsv.join("\n") + "\n",
        msl,
        log,
        shaders: kept.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_glsl() {
        assert!(is_standalone_glsl(b"void main ( void ) { }"));
        assert!(!is_standalone_glsl(b"// void main() {\nfloat f();"));
        assert!(!is_standalone_glsl(b"void main() {\x01}"));
        assert!(!is_standalone_glsl(b""));
    }

    #[test]
    fn call_args_and_unbalanced() {
        let t = "textureOffset(s, f(a, b), ivec2(o))";
        let (a, end) = call_args(t, 13).unwrap();
        assert_eq!(a, ["s", " f(a, b)", " ivec2(o)"]);
        assert_eq!(end, t.len());
        assert!(call_args("f(a", 1).is_none());
    }

    #[test]
    fn int_base_zero() {
        assert_eq!(int0("0x1F"), Some(31));
        assert_eq!(int0("00"), Some(0));
        assert_eq!(int0("010"), None);
        assert_eq!(int0("7"), Some(7));
    }

    #[test]
    fn splitext_as_python() {
        assert_eq!(splitext("a.vs"), ".vs");
        assert_eq!(splitext(".vs"), "");
        assert_eq!(splitext("a..fs"), ".fs");
        assert_eq!(splitext("noext"), "");
    }
}
