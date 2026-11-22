//! `finish(key, w, j, c1)` of `dmsh_build.py` (l.737-875) and everything it calls after the translator: spirv-cross, the second translation pass, the post-rewrite chain
//! (`crate::post_rewrites`), the reflection json, the pixel shaders' side libraries, the vertex shaders' shadow-input and pulling libraries, the geometry shaders.
//! API and status of every piece: `docs/shaders/SHADER-FINISH-RUST.md`.
//!
//! The Rust function reproduces what `shader_device.build` makes: **iOS only, source mode**: the libraries are the MSL text written under the library file names, and the
//! returned `outputs` are the files of the final image.
pub mod abi;
pub mod entry;
pub mod fetch;
pub mod geometry;
pub mod lean;
pub mod mesh;
pub mod meshstage;
pub mod pycompat;
pub mod pyjson;
pub mod reflect;
pub mod rx;
pub mod spv;
pub mod tess;
mod tess_text;
pub mod texts;
pub mod unit;
pub mod vin;

use crate::tools::ShaderTools;
use crate::{Job, PostError, Stage, post_rewrites};
use pyjson::J;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

pub use reflect::user_locations;

/// `ios_metal.RECORD` of the engine copy this was ported from
pub const IOS_RECORD: &str = "xcrun -sdk iphoneos metal -c -O1 -mios-version-min=18.0 -fmetal-math-mode=relaxed -fpreserve-invariance -fwrapv (-std=metal4.0 for a source using vertex_value); xcrun -sdk iphoneos metallib";
/// `dmsh_mtlcache.FORMAT`
const MTLCACHE_FORMAT: &[u8] = b"dmsh_mtlcache 2";
/// `mtl.compiler_id = lambda sdk: 'source-mode ' + sdk` (shader_device._source_mode)
const SOURCE_MODE_COMPILER: &str = "source-mode iphoneos";

/// What `dmsh_build.py` reads from its environment (iOS, source mode)
#[derive(Clone, Debug)]
pub struct Options {
    /// `DMSH_IOS_PULL`: also write the pulling libraries `<key>_bp.metal`; the reflection's `pull` entry is written either way
    pub pulling: bool,
    /// `ios_metal.RECORD`: the reflection's `chain_metal`
    pub chain_metal: String,
    /// `Cache.ident['ios'].hex()`: the reflection's `chain_compiler`
    pub chain_compiler: String,
}

impl Options {
    /// the values `shader_device.build` produces: `sha256(FORMAT \0 record \0 'source-mode iphoneos')`
    pub fn ios_source_mode(pulling: bool) -> Options {
        let mut h = Sha256::new();
        h.update(MTLCACHE_FORMAT);
        h.update(b"\0");
        h.update(IOS_RECORD.as_bytes());
        h.update(b"\0");
        h.update(SOURCE_MODE_COMPILER.as_bytes());
        let d = h.finalize();
        Options {
            pulling,
            chain_metal: IOS_RECORD.to_string(),
            chain_compiler: d.iter().map(|b| format!("{b:02x}")).collect(),
        }
    }
}

/// The tables `dmsh_build.py` reads at module level; loaded once per build
pub struct Ctx {
    pub chain: String,
    pub opts: Options,
    loc: HashMap<String, (String, i64)>,
    pc_of: HashMap<String, String>,
    pairs: HashMap<String, BTreeSet<String>>,
    bf_want: BTreeSet<String>,
    dual_want: BTreeSet<String>,
    empty: BTreeSet<String>,
}

fn read_text(p: &Path) -> Option<String> {
    std::fs::read(p)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

impl Ctx {
    /// `profile_dir`: the profile folder `shader_device.build` copies (a folder holding game.tsv, else its `<game_id>` sub-folder), `extra_dir`: where `pso_pairs.tsv` is looked for when the profile has none
    pub fn load(
        profile_dir: &Path,
        game_id: &str,
        chain: &str,
        opts: Options,
    ) -> Result<Ctx, String> {
        Ctx::load_with(profile_dir, game_id, chain, opts, None)
    }
    pub fn load_with(
        profile_dir: &Path,
        game_id: &str,
        chain: &str,
        opts: Options,
        extra_dir: Option<&Path>,
    ) -> Result<Ctx, String> {
        let dir: PathBuf = if profile_dir.join("game.tsv").is_file() {
            profile_dir.to_path_buf()
        } else {
            profile_dir.join(game_id)
        };
        let map = read_text(&dir.join("shader_map.tsv"))
            .ok_or_else(|| format!("{}: no shader_map.tsv", dir.display()))?;
        let conflicts = read_text(&dir.join("shader_map.tsv.conflicts.tsv")).unwrap_or_default();
        let mut pair_texts = Vec::new();
        if let Some(t) = read_text(&dir.join("shader_pairs.tsv")) {
            pair_texts.push(t);
        }
        match read_text(&dir.join("pso_pairs.tsv")) {
            Some(t) => pair_texts.push(t),
            None => {
                if let Some(d) = extra_dir
                    && let Some(t) = read_text(&d.join("pso_pairs.tsv"))
                {
                    pair_texts.push(t);
                }
            }
        }
        let refs: Vec<&str> = pair_texts.iter().map(|s| s.as_str()).collect();
        Ok(Ctx::from_tables(
            chain,
            opts,
            &map,
            &conflicts,
            &refs,
            &read_text(&dir.join("bf_want.tsv")).unwrap_or_default(),
            &read_text(&dir.join("dual_want.tsv")).unwrap_or_default(),
        ))
    }

    /// the same from tables the caller holds (`pairs`: the texts of shader_pairs.tsv and pso_pairs.tsv)
    pub fn from_tables(
        chain: &str,
        opts: Options,
        shader_map: &str,
        conflicts: &str,
        pairs: &[&str],
        bf_want: &str,
        dual_want: &str,
    ) -> Ctx {
        let (mut loc, mut pc_of): (HashMap<String, (String, i64)>, HashMap<String, String>) =
            (HashMap::new(), HashMap::new());
        for ln in shader_map.lines() {
            let c: Vec<&str> = ln.split('\t').collect();
            if c.len() > 4 && c[0].ends_with(".bin") {
                pc_of
                    .entry(c[3].to_string())
                    .or_insert_with(|| c[4].to_string());
                loc.entry(c[3].to_string())
                    .or_insert_with(|| (c[0].to_string(), c[1].trim().parse().unwrap_or(0)));
            }
        }
        for ln in conflicts.lines() {
            let c: Vec<&str> = ln.split('\t').collect();
            if c.len() > 1
                && !ln.starts_with('#')
                && !ln.starts_with("dm_")
                && pc_of.contains_key(c[0])
            {
                pc_of.insert(c[0].to_string(), c[1].to_string());
            }
        }
        let mut pm: HashMap<String, BTreeSet<String>> = HashMap::new();
        for t in pairs {
            for ln in t.lines() {
                if !ln.trim().is_empty() && !ln.starts_with('#') {
                    let mut it = ln.split_whitespace();
                    if let (Some(v), Some(p)) = (it.next(), it.next()) {
                        pm.entry(v.to_string()).or_default().insert(p.to_string());
                    }
                }
            }
        }
        let bf: BTreeSet<String> = bf_want
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .map(|l| l.trim().to_string())
            .collect();
        let dual: BTreeSet<String> = dual_want
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .filter_map(|l| l.split_whitespace().next())
            .map(|s| s.to_string())
            .collect();
        Ctx {
            chain: chain.to_string(),
            opts,
            loc,
            pc_of,
            pairs: pm,
            bf_want: bf,
            dual_want: dual,
            empty: BTreeSet::new(),
        }
    }

    /// the build's two phases: shaders without pairs first, then the vertex shaders that have pairs
    pub fn is_paired(&self, key: &str) -> bool {
        self.pairs.contains_key(key)
    }
    pub fn pairs_of(&self, key: &str) -> &BTreeSet<String> {
        self.pairs.get(key).unwrap_or(&self.empty)
    }
    pub fn has_row(&self, key: &str) -> bool {
        self.loc.contains_key(key)
    }
    pub fn source_of(&self, key: &str) -> Option<String> {
        self.loc.get(key).map(|(n, k)| format!("{n}#{k}"))
    }
}

/// What `finish` made of one shader
#[derive(Debug, Default, Clone)]
pub struct Finished {
    /// "ok", the Python's failure kind verbatim, "unported: <why>", or "fatal: <msg>" where the Python `sys.exit`s or raises
    pub status: String,
    pub detail: String,
    /// the `refl` dict `finish` returns (before `with_compiler`)
    pub refl: Option<Value>,
    /// library file name -> bytes: the shader's files of the final image
    pub outputs: Vec<(String, Vec<u8>)>,
    /// the `user(locnN)` numbers of `<key>_b.metal` (pixel shaders): what a paired vertex shader's `vs_pad` reads
    pub ps_locs: BTreeSet<u32>,
    /// the `[RED] ...` lines the Python prints for what a shader lost
    pub log: Vec<String>,
}

impl Finished {
    fn unported(why: impl Into<String>) -> Finished {
        let w = why.into();
        Finished {
            status: format!("unported: {w}"),
            detail: w,
            ..Default::default()
        }
    }
    fn fail(status: &str, detail: impl Into<String>) -> Finished {
        Finished {
            status: status.to_string(),
            detail: detail.into(),
            ..Default::default()
        }
    }
    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }
    pub fn is_unported(&self) -> bool {
        self.status.starts_with("unported")
    }
}

/// `s[:160]` of Python (characters)
pub(crate) fn t160(s: &str) -> String {
    s.chars().take(160).collect()
}

/// What `finish` needs to know of the shaders paired with the one it finishes (`PAIRS`): a vertex shader's `vs_pad` reads their finished MSL's `user(locnN)` numbers, a mesh shader's
/// `partner_reads` also their SPH input maps.
pub trait Partners {
    /// the `user(locnN)` numbers of the paired pixel shader's finished `<ps>_b.metal`; None: that shader was not built (the pair is skipped)
    fn locs(&self, ps: &str) -> Option<BTreeSet<u32>>;
    /// `dmsh_meshstage.pixel_reads` of the paired pixel shader's SPH (`partner_reads`), asked only for a pair whose shader is in the shader map and only when finishing a mesh shader
    fn sph_reads(&self, _ps: &str) -> SphReads {
        SphReads::Unsupported
    }
}

/// the answer to [`Partners::sph_reads`]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SphReads {
    /// {generic attribute: the mask of the components the pixel shader reads}
    Reads(std::collections::BTreeMap<i64, i64>),
    /// the Python raises there (a record whose SPH is shorter than its input map): its build stops
    Failed(String),
    /// the caller cannot read the shader's record: a mesh shader with pairs is `unported`
    Unsupported,
}

struct LocsOnly<'a>(&'a dyn Fn(&str) -> Option<BTreeSet<u32>>);
impl Partners for LocsOnly<'_> {
    fn locs(&self, ps: &str) -> Option<BTreeSet<u32>> {
        (self.0)(ps)
    }
}

/// `finish(key, w, j, c1)`; `paired_ps(ps key)` = the finished `<ps>_b.metal` text of a pixel shader paired with this vertex shader (None: not available, the pair is skipped)
pub fn finish(
    ctx: &Ctx,
    key: &str,
    work: &Path,
    job: &mut Value,
    c1: &[u8],
    tools: &dyn ShaderTools,
    paired_ps: &dyn Fn(&str) -> Option<String>,
) -> Finished {
    finish_with_locs(ctx, key, work, job, c1, tools, &|ps| {
        paired_ps(ps).map(|m| user_locations(&m))
    })
}

/// the same, with the paired pixel shaders' `user(locnN)` sets (`Finished::ps_locs`) instead of their MSL. A mesh shader with pairs cannot be finished through this entry point
/// (it needs the pixel shaders' SPH input maps: [`finish_partners`]); it is `unported`
pub fn finish_with_locs(
    ctx: &Ctx,
    key: &str,
    work: &Path,
    job: &mut Value,
    c1: &[u8],
    tools: &dyn ShaderTools,
    paired_locs: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
) -> Finished {
    finish_partners(ctx, key, work, job, c1, tools, &LocsOnly(paired_locs))
}

/// `finish(key, w, j, c1)` with the paired shaders given as [`Partners`]
pub fn finish_partners(
    ctx: &Ctx,
    key: &str,
    work: &Path,
    job: &mut Value,
    c1: &[u8],
    tools: &dyn ShaderTools,
    partners: &dyn Partners,
) -> Finished {
    let mut log = Vec::new();
    let mut f = match finish_inner(ctx, key, work, job, c1, tools, partners, &mut log) {
        Ok(f) => f,
        Err(f) => f,
    };
    f.log = log;
    f
}

type R<T> = Result<T, Finished>;

fn io_err(e: std::io::Error, what: &str) -> Finished {
    Finished::fail("fatal", format!("{what}: {e}"))
}

#[allow(clippy::result_large_err)] // Err is the finished-job record (flow control); boxing it would reshape every caller
fn read(p: &Path) -> R<Vec<u8>> {
    std::fs::read(p).map_err(|e| io_err(e, &p.display().to_string()))
}
#[allow(clippy::result_large_err)] // Err is the finished-job record (flow control); boxing it would reshape every caller
fn write(p: &Path, b: &[u8]) -> R<()> {
    std::fs::write(p, b).map_err(|e| io_err(e, &p.display().to_string()))
}
fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn jstr<'a>(j: &'a Value, k: &str) -> Option<&'a str> {
    j.get(k).and_then(|v| v.as_str())
}

#[allow(clippy::result_large_err)] // Err is the finished-job record (flow control); boxing it would reshape every caller
fn run_cross(tools: &dyn ShaderTools, key: &str, work: &Path) -> R<String> {
    let spv = work.join(format!("{key}.spv"));
    let msl = work.join(format!("{key}.metal"));
    let data = read(&spv)?;
    let mut args = spv::cross_args(&data, &s(&spv)).map_err(|e| Finished::fail("fatal", e))?;
    args.push(s(&spv));
    args.push("--output".into());
    args.push(s(&msl));
    let r = tools.spirv_cross(&args);
    if r.rc != 0 {
        return Err(Finished::fail("MSL failed", t160(r.stderr.trim())));
    }
    let text = read(&msl)?;
    Ok(String::from_utf8_lossy(&text).into_owned())
}

fn chain_err(e: PostError) -> Finished {
    match &e {
        PostError::Unported(step) => Finished::unported(format!("chain step {step}")),
        PostError::Failed { step, msg } => match e.status() {
            "exception" => Finished::fail("fatal", format!("{step}: {msg}")),
            st => Finished::fail(
                st,
                if *step == "check_msl" {
                    msg.clone()
                } else {
                    t160(msg)
                },
            ),
        },
    }
}

static RE_ENTRY_KIND: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(vertex|fragment|kernel)\s+\S+\s+(\w+)\s*\(").expect("static regex")
});

#[allow(clippy::too_many_arguments)]
#[allow(clippy::result_large_err)] // Err is the finished-job record (flow control); boxing it would reshape every caller
fn finish_inner(
    ctx: &Ctx,
    key: &str,
    work: &Path,
    job: &mut Value,
    c1: &[u8],
    tools: &dyn ShaderTools,
    partners: &dyn Partners,
    log: &mut Vec<String>,
) -> R<Finished> {
    let stage = jstr(job, "stage").unwrap_or("?").to_string();
    let Some(source) = ctx.source_of(key) else {
        return Err(Finished::unported(format!(
            "{key} is not in the shader map"
        )));
    };
    // `mesh = j.get('mesh_stage')`: a non-empty dict is a task or mesh shader (translated as compute, wrapped by `dmsh_meshstage.to_stage`)
    let mesh_json: Option<Value> = job
        .get("mesh_stage")
        .filter(|v| match v {
            Value::Null => false,
            Value::Object(o) => !o.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Bool(b) => *b,
            _ => true,
        })
        .cloned();
    match stage.as_str() {
        "ps" | "vs" | "cs" | "hs" | "ds" => {}
        "gs" => return geometry::finish_gs(ctx, key, work, job, c1, tools, &source, log),
        other => return Err(Finished::unported(format!("stage {other}"))),
    }
    let spv_path = work.join(format!("{key}.spv"));
    let msl_path = work.join(format!("{key}.metal"));
    let vs = stage == "vs";
    let ds = stage == "ds";
    if vs || ds {
        single_lane(&spv_path)?;
    } // one-lane subgroups; a domain shader is a vertex function in Metal too
    let mut src = run_cross(tools, key, work)?;
    if vs {
        // second pass: re-translate with inputs that are only used as integers reported as uint
        let attr_has = |job: &Value, l: &str| job.get("attr").and_then(|a| a.get(l)).is_some();
        let extra: BTreeMap<String, String> = entry::int_inputs(&src)
            .into_iter()
            .filter(|(l, _)| !attr_has(job, l))
            .collect();
        if !extra.is_empty() {
            if let Some(attr) = job.get_mut("attr").and_then(|a| a.as_object_mut()) {
                for (l, t) in &extra {
                    attr.insert(l.clone(), Value::String(t.clone()));
                }
            } else if let Some(o) = job.as_object_mut() {
                o.insert(
                    "attr".into(),
                    Value::Object(
                        extra
                            .iter()
                            .map(|(l, t)| (l.clone(), Value::String(t.clone())))
                            .collect(),
                    ),
                );
            }
            let jp = work.join(format!("job1_{key}.json"));
            write(
                &jp,
                &serde_json::to_vec(&Value::Array(vec![job.clone()])).unwrap_or_default(),
            )?;
            let rx = tools.xlate(&[s(&jp), s(work)]);
            if rx.rc != 0 || !spv_path.exists() {
                return Err(Finished::fail(
                    "translation failed",
                    t160(&format!(
                        "integer-input retranslation: {}",
                        rx.stderr.trim()
                    )),
                ));
            }
            single_lane(&spv_path)?;
            src = run_cross(tools, key, work)?;
        }
    }
    let res: Value = serde_json::from_slice(&read(&work.join(format!("{key}.res.json")))?)
        .map_err(|e| Finished::fail("fatal", format!("{key}.res.json: {e}")))?;
    let local: Option<Vec<i64>> = job
        .get("local")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect());
    let has_tf = job
        .get("tf")
        .map(|v| match v {
            Value::Null => false,
            Value::Array(a) => !a.is_empty(),
            Value::Bool(b) => *b,
            _ => true,
        })
        .unwrap_or(false);
    let paired = ctx.is_paired(key) && vs;
    let partner: Option<BTreeSet<u32>> = if paired {
        let mut want = BTreeSet::new();
        for ps in ctx.pairs_of(key) {
            if let Some(l) = partners.locs(ps) {
                want.extend(l);
            }
        }
        Some(want)
    } else {
        None
    };
    let jb = Job {
        stage: Stage::parse(&stage),
        local: local.clone(),
        has_tf,
        tf: job.get("tf").filter(|_| has_tf),
        dual_want: ctx.dual_want.contains(key),
        partner_locs: partner.as_ref(),
        res: Some(&res),
        mesh: mesh_json.is_some(),
    };
    let post = post_rewrites(&src, &jb).map_err(chain_err)?;
    // the source the reflection reads: the one with only main0 (`src` after vs_depth / before the shared body of a pixel shader); the chain says when it is not `out` itself
    let mut analysis: String = post.analysis.clone().unwrap_or_else(|| post.out.clone());
    let mut msl_text = post.out.clone();
    write(&msl_path, msl_text.as_bytes())?;
    // hull as a plain kernel, domain as a plain vertex function on the host's grid (dmsh_tess.py); an unsupported shape keeps the source and says so in the reflection
    let mut tess: Option<J> = None;
    let mut mesh: Option<meshstage::MeshInfo> = None;
    if (stage == "hs" || stage == "ds") && mesh_json.is_none() {
        let spv_bytes = read(&spv_path)?;
        let r = if stage == "hs" {
            tess::hull(&msl_text, &spv_bytes).map(|(s, f)| (s, f.to_j()))
        } else {
            tess::domain(
                &msl_text,
                &spv_bytes,
                job.get("tess_mode")
                    .and_then(tess::Mode::from_json)
                    .as_ref(),
            )
            .map(|(s, f)| (s, f.to_j()))
        };
        match r {
            Ok((s, facts)) => {
                msl_text = s;
                analysis = msl_text.clone();
                write(&msl_path, msl_text.as_bytes())?;
                tess = Some(facts);
            }
            Err(e) if e.starts_with(tess::CRASH) => {
                return Err(Finished::fail(
                    "fatal",
                    format!("dmsh_tess: {}", tess::plain(&e)),
                ));
            } // an exception the Python's finish does not catch
            Err(e) => tess = Some(J::obj().with("unsupported", t160(&e))),
        }
    }
    if let Some(mj) = &mesh_json {
        // the kernel as the Metal object or mesh function (dmsh_meshstage.py)
        let mut mi = meshstage::MeshInfo::from_json(mj)
            .map_err(|e| Finished::fail("fatal", format!("mesh_stage: {e}")))?;
        if let Some(r) = &mi.refused {
            return Err(Finished::fail("mesh stage failed", r.clone()));
        }
        let reads = partner_reads(ctx, key, partners)?;
        match meshstage::to_stage(&msl_text, &mut mi, reads.as_ref()) {
            Ok(t) => {
                msl_text = t;
                analysis = msl_text.clone();
                write(&msl_path, msl_text.as_bytes())?;
            }
            Err(e) => return Err(Finished::fail("mesh stage failed", t160(&e))),
        }
        mesh = Some(mi);
    }
    if ds && matches!(&tess, Some(t) if !t.has("unsupported")) {
        // the domain is the last stage before the rasterizer: the two depth-convention entry points, and its capture entry for a geometry shader after it
        let capture = crate::vs::capture(&msl_text).map_err(chain_err)?;
        let (single, full) = crate::vs::vs_depth(&msl_text).map_err(chain_err)?;
        msl_text = format!("{full}{capture}");
        analysis = single;
        write(&msl_path, msl_text.as_bytes())?;
    }
    let rstage: String = mesh
        .as_ref()
        .map(|m| m.kind.clone())
        .unwrap_or_else(|| stage.clone());
    let binds = reflect::reflect(key, &rstage, &analysis, &res)
        .map_err(|e| Finished::fail("reflection failed", t160(&e)))?;
    if vs {
        // the chain decides `main0_discard` from an approximation of the reflection's `rw`; with the real bindings in hand, check it
        let exact = binds
            .iter()
            .any(|b| b.get("rw").map(|r| r.is_truthy()).unwrap_or(false))
            || crate::vs::vs_writes(&analysis, false);
        if exact != post.entry_discard {
            return Err(Finished::unported(
                "the chain's vs_writes approximation disagrees with the reflection",
            ));
        }
    }
    let mut outputs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut side = J::obj();
    if stage == "ps" {
        side = ps_libraries(ctx, key, &msl_text, &binds, &mut outputs);
    }
    outputs.push((format!("{key}_b.metal"), msl_text.clone().into_bytes()));
    let entry = RE_ENTRY_KIND.captures(&analysis);
    let dr = reflect::drv_reads(&analysis);
    let omap = if stage == "ps" {
        let code = std::fs::read(work.join(format!("{key}.bin")))
            .or_else(|_| {
                jstr(job, "code")
                    .map(std::fs::read)
                    .unwrap_or_else(|| Err(std::io::ErrorKind::NotFound.into()))
            })
            .map_err(|e| io_err(e, "code"))?;
        Some(if code.len() >= 0x50 {
            J::Int(u32::from_le_bytes([code[72], code[73], code[74], code[75]]) as i64)
        } else {
            J::Null
        })
    } else {
        None
    };
    let mut refl = J::obj()
        .with("key", format!("{key}_b"))
        .with("route", "B")
        .with("chain", ctx.chain.as_str())
        .with("dm_ctl", key)
        .with("source", source.as_str())
        .with("pc_paired", ctx.pc_of.get(key).map(|s| s.as_str()))
        .with("stage", rstage.as_str());
    if let Some(m) = &mesh {
        refl.set(
            "mesh_stage",
            J::obj()
                .with("kind", m.kind.as_str())
                .with("threads", m.threads)
                .with("payload", m.payload)
                .with("task", m.task.unwrap_or(true)),
        );
        if m.kind == "ms" {
            refl.set("entry_z", "main0_z");
        }
        if m.kind == "ms" && !m.task.unwrap_or(true) {
            refl.set("entry_object", "main0_o");
        }
        if m.spill {
            refl.set("mesh_spill", 1);
        }
    }
    refl.set(
        "entry",
        entry
            .as_ref()
            .map(|m| m[2].to_string())
            .unwrap_or_else(|| "main0".into()),
    );
    refl.set("msl_entry_kind", entry.as_ref().map(|m| m[1].to_string()));
    if vs {
        refl.set("entry_z", "main0_z");
        refl.set("entry_geometry_capture", "main0_gs");
    }
    if ds && matches!(&tess, Some(t) if !t.has("unsupported")) {
        refl.set("entry_z", "main0_z");
        refl.set("entry_geometry_capture", "main0_gs");
    }
    if vs && crate::vs::layer_out(&analysis) {
        refl.set("layer_out", true);
    }
    if post.entry_discard {
        refl.set("entry_discard", "main0_discard");
    }
    if has_tf {
        let strides: Vec<J> = job
            .get("tf")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .map(|e| {
                        e.get("stride")
                            .and_then(|x| x.as_i64())
                            .map(J::Int)
                            .unwrap_or(J::Int(0))
                    })
                    .collect()
            })
            .unwrap_or_default();
        refl.set("entry_tf", "main0_tf");
        refl.set("tf_strides", J::Arr(strides));
    }
    if let Some((loc, comp)) = post.layer_from {
        refl.set("entry_layer", "main0_l");
        refl.set("entry_layer_z", "main0_lz");
        refl.set(
            "layer_from",
            J::obj().with("loc", loc).with("comp", comp.to_string()),
        );
    }
    if post.fs_uint {
        refl.set("entry_u", "main0_u");
    }
    if post.fs_dual {
        refl.set("entry_d", "main0_d");
    }
    if post.blend_zero > 0 {
        refl.set("blend_zero", true);
    }
    if !post.first_store.is_empty() {
        let mut fs = Vec::new();
        for (n, w, v) in &post.first_store {
            let Some(reg) = binds
                .iter()
                .find(|b| b.get("msl_name").and_then(|x| x.as_str()) == Some(n.as_str()))
                .and_then(|b| b.get("reg").cloned())
            else {
                return Err(Finished::fail(
                    "fatal",
                    format!("first_store: no binding {n}"),
                ));
            };
            fs.push(J::obj().with("reg", reg).with("word", *w).with("value", *v));
        }
        refl.set("first_store", J::Arr(fs));
    }
    if post.tg_spill > 0 {
        // the json's integers are i64: a total past it (saturated by tg_scan) is written as i64::MAX, never as a negative number
        refl.set("tg_spill", i64::try_from(post.tg_spill).unwrap_or(i64::MAX));
    }
    refl.set(
        "threadgroup_size",
        match &local {
            Some(l) => J::Arr(l.iter().map(|x| J::Int(*x)).collect()),
            None => job.get("local").map(J::from_value).unwrap_or(J::Null),
        },
    );
    refl.set(
        "c1",
        c1.iter().map(|b| format!("{b:02x}")).collect::<String>(),
    );
    let mut notes: Vec<J> = res
        .get("notes")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().map(J::from_value).collect())
        .unwrap_or_default();
    if job
        .get("no_facts")
        .map(|v| !v.is_null() && v != &Value::Bool(false))
        .unwrap_or(false)
    {
        notes.push(J::from("not in the answer table, queries use defaults"));
    }
    refl.set("notes", J::Arr(notes));
    refl.set("drv_reads", dr.clone());
    refl.set(
        "drv_unfillable",
        reflect::unfillable(&dr, &rstage)
            .into_iter()
            .map(J::Str)
            .collect::<Vec<_>>(),
    );
    refl.set("bindings", J::Arr(binds.clone()));
    refl.set(
        "vertex_inputs",
        J::Arr(
            reflect::attributes(&analysis)
                .into_iter()
                .map(|(t, _, l)| J::obj().with("location", l).with("msl_type", t))
                .collect(),
        ),
    );
    let mut raster = reflect::raster_facts(&stage, &analysis);
    if let Some(o) = omap {
        raster.set("omap", o);
    }
    refl.set("raster", raster);
    refl.set(
        "vs_pad",
        J::Arr(post.vs_pad.iter().map(|x| J::Int(*x as i64)).collect()),
    );
    if let Some(t) = &tess {
        refl.set("tess", t.clone());
    }
    if vs {
        vin_finish(&msl_text, &mut refl, &mut outputs, key, log);
        pull_finish(ctx, &msl_text, &mut refl, &mut outputs, key, log);
    }
    refl.extend(&side);
    let ps_locs = if stage == "ps" {
        user_locations(&msl_text)
    } else {
        BTreeSet::new()
    };
    Ok(finalize(ctx, key, refl, outputs, ps_locs))
}

/// `partner_reads(key)`: {location: component mask} the pixel shaders paired with a mesh shader read (`dmsh_meshstage.to_stage`'s reads), from their SPH input maps; None without pairs.
/// A location a partner's MSL declares that its input map does not name counts as all four.
#[allow(clippy::result_large_err)] // Err is the finished-job record (flow control); boxing it would reshape every caller
fn partner_reads(
    ctx: &Ctx,
    key: &str,
    partners: &dyn Partners,
) -> R<Option<std::collections::BTreeMap<i64, i64>>> {
    let pairs = ctx.pairs_of(key);
    if pairs.is_empty() {
        return Ok(None);
    }
    let mut reads: std::collections::BTreeMap<i64, i64> = Default::default();
    for ps in pairs {
        let mut got: std::collections::BTreeMap<i64, i64> = Default::default();
        if ctx.has_row(ps) {
            match partners.sph_reads(ps) {
                SphReads::Reads(m) => got = m,
                SphReads::Failed(e) => {
                    return Err(Finished::fail("fatal", format!("partner_reads {ps}: {e}")));
                }
                SphReads::Unsupported => {
                    return Err(Finished::unported(
                        "a mesh shader paired with pixel shaders needs their SPH input maps (finish_partners with Partners::sph_reads)",
                    ));
                }
            }
        }
        for n in partners.locs(ps).unwrap_or_default() {
            got.entry(n as i64).or_insert(0xf);
        }
        for (n, m) in got {
            *reads.entry(n).or_insert(0) |= m;
        }
    }
    Ok(Some(reads))
}

/// `write_refl`: `with_compiler('ios', refl)` and `atomic_json` (`json.dump(indent=1, ensure_ascii=False)`)
fn finalize(
    ctx: &Ctx,
    key: &str,
    refl: J,
    mut outputs: Vec<(String, Vec<u8>)>,
    ps_locs: BTreeSet<u32>,
) -> Finished {
    let mut w = refl.clone();
    w.set("chain_metal", ctx.opts.chain_metal.as_str());
    w.set("chain_compiler", ctx.opts.chain_compiler.as_str());
    outputs.push((format!("{key}_b.json"), w.dump().into_bytes()));
    Finished {
        status: "ok".into(),
        detail: String::new(),
        refl: Some(refl.to_value()),
        outputs,
        ps_locs,
        log: vec![],
    }
}

/// a result whose files (the json included) the caller has already written (geometry shaders: `save` writes each variant's json itself)
fn finalize_with(
    _ctx: &Ctx,
    _key: &str,
    refl: J,
    outputs: Vec<(String, Vec<u8>)>,
    ps_locs: BTreeSet<u32>,
) -> Finished {
    Finished {
        status: "ok".into(),
        detail: String::new(),
        refl: Some(refl.to_value()),
        outputs,
        ps_locs,
        log: vec![],
    }
}

#[allow(clippy::result_large_err)] // Err is the finished-job record (flow control); boxing it would reshape every caller
fn single_lane(p: &Path) -> R<()> {
    let d = read(p)?;
    if let Some(n) = spv::spv_single_lane(&d).map_err(|e| Finished::fail("fatal", e))? {
        write(p, &n)?;
    }
    Ok(())
}

/// `ps_libraries` in source mode: the unit's text goes under every cut name (`_bx`, `_bf`); `_b` is the plain source (the caller adds it)
fn ps_libraries(
    ctx: &Ctx,
    key: &str,
    full: &str,
    binds: &[J],
    outputs: &mut Vec<(String, Vec<u8>)>,
) -> J {
    let (src, info) = unit::unit(full);
    let side = src.is_some() && (!info.alpha.is_empty() || !info.logic.is_empty());
    let folded = fetch::fold(
        src.as_deref().unwrap_or(full),
        binds,
        ctx.bf_want.contains(key),
    );
    let candidates: Vec<(String, Option<J>)> = match (&folded, &src) {
        (Ok((f, m)), _) => vec![(f.clone(), Some(m.clone()))],
        (Err(_), Some(u)) => vec![(u.clone(), None)],
        _ => vec![],
    };
    if let Some((unit_text, fetch_meta)) = candidates.into_iter().next() {
        if side {
            outputs.push((format!("{key}_bx.metal"), unit_text.clone().into_bytes()));
        }
        if fetch_meta.is_some() {
            outputs.push((format!("{key}_bf.metal"), unit_text.into_bytes()));
        }
        let mut r = J::obj();
        if side {
            if !info.alpha.is_empty() {
                r.set(
                    "alpha",
                    J::obj().with(
                        "entries",
                        J::Arr(info.alpha.iter().map(|s| J::from(s.as_str())).collect()),
                    ),
                );
            }
            if !info.logic.is_empty() {
                r.set(
                    "logic",
                    J::obj().with(
                        "entries",
                        J::Arr(info.logic.iter().map(|s| J::from(s.as_str())).collect()),
                    ),
                );
            }
        }
        if let Some(m) = fetch_meta {
            r.set("fetch", m);
        }
        return r;
    }
    J::obj()
}

fn vin_finish(
    full: &str,
    refl: &mut J,
    outputs: &mut Vec<(String, Vec<u8>)>,
    key: &str,
    log: &mut Vec<String>,
) {
    refl.remove("vin");
    match vin::vin_variant(full) {
        Ok((v, shadows)) => {
            outputs.push((format!("{key}_bv.metal"), v.into_bytes()));
            refl.set(
                "vin",
                J::obj()
                    .with("entry", "main0")
                    .with("entry_z", "main0_z")
                    .with("shadow", J::Arr(shadows)),
            );
        }
        Err(why) => {
            if why != "no integer input" {
                log.push(format!("[RED] {key}: no shadow-input library ({why}): its integer inputs keep the CPU conversion"));
            }
        }
    }
}

fn pull_finish(
    ctx: &Ctx,
    full: &str,
    refl: &mut J,
    outputs: &mut Vec<(String, Vec<u8>)>,
    key: &str,
    log: &mut Vec<String>,
) {
    refl.remove("pull");
    match vin::pull_variant(full) {
        Ok((v, info)) => {
            if ctx.opts.pulling {
                outputs.push((format!("{key}_bp.metal"), v.into_bytes()));
            }
            let mut p = J::obj();
            if let J::Obj(m) = &info {
                for (k, v) in m {
                    if *v != J::Null {
                        p.set(k, v.clone());
                    }
                }
            }
            refl.set("pull", p);
        }
        Err(why) => {
            if why != "no stage input" {
                log.push(format!("[RED] {key}: no pulling library ({why}): its pipelines are always built for the draw's vertex layout"));
            }
        }
    }
}
