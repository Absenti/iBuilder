//! The post-rewrite chain (`tools/shader_post.py post_rewrites` = `dmsh_build.py finish()` between spirv-cross and the compile), for ps / vs / cs.
//!
//! Order and conditions are those of `finish()` (dmsh_build.py l.762-868). Every step of the Python for these stages is ported except `hmma_msl`, which returns `Unported`
//! for a shader that defines the HMMA helper (see `hmma.rs`).
use crate::fsuint::{fs_dual, fs_uint};
use crate::{
    PostError, ampere, bit0, blend, border, cs, guards, hmma, pervertex, pool, side, steps, utest,
    vs, wide, zsnap,
};
use std::borrow::Cow;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Stage {
    #[default]
    Ps,
    Vs,
    Cs,
    Hs,
    Ds,
    Other(String),
}
impl Stage {
    pub fn parse(s: &str) -> Stage {
        match s {
            "ps" => Stage::Ps,
            "vs" => Stage::Vs,
            "cs" => Stage::Cs,
            "hs" => Stage::Hs,
            "ds" => Stage::Ds,
            o => Stage::Other(o.to_string()),
        }
    }
}

#[derive(Default)]
pub struct Job<'a> {
    pub stage: Stage,
    /// the compute thread group `[x, y, z]` of the answer table
    pub local: Option<Vec<i64>>,
    /// `bool(j.get('tf'))`: a transform feedback layout is present. When true the layout itself must be in `tf` (a vertex shader that has none gets no capture entry point: `Unported`)
    pub has_tf: bool,
    /// `j['tf']`: the four feedback streams, as `dmsh_build.tf_layout` returns them (`tf::tf_layout`)
    pub tf: Option<&'a serde_json::Value>,
    /// the key is in the profile's dual_want.tsv
    pub dual_want: bool,
    /// `PAIRS.get(key)` non-empty: the `user(locnN)` numbers the paired pixel shaders' MSL reads (None: the vertex shader has no pairs)
    pub partner_locs: Option<&'a BTreeSet<u32>>,
    /// the translator's answer json (`<key>.res.json`), read by the vertex shaders' reflection (`rw` of storage buffers)
    pub res: Option<&'a serde_json::Value>,
    /// a task or mesh shader (a job with `mesh_stage`; its stage is `cs`): the compute tail of `finish()` (`first_store`, `tg_spill`, `max_threads`) does not run, `dmsh_meshstage.to_stage` replaces it
    pub mesh: bool,
}

#[derive(Debug, Default, Clone)]
pub struct PostOut {
    /// the text `finish()` writes to `<key>.metal` (and the library's `_b.metal`): with the extra entry points
    pub out: String,
    /// the source the reflection reads (`src` of `finish()` after the chain, before the extra entry points): None when it is `out` itself (cs; a pixel shader without a shared body)
    /// vertex shader: main0 only, with the GL depth rewrite (`vs_depth`'s first result); pixel shader with a shared body: `main0` as it was before `dm_body_main0`
    pub analysis: Option<String>,
    /// vertex shader: `out[..plain_len]` is `plain` (the source without the layer entry points, which a library that does not build falls back to)
    pub plain_len: Option<usize>,
    pub blend_zero: u32,
    pub fs_uint: bool,
    pub fs_dual: bool,
    pub shared_body: bool,
    pub vs_pad: Vec<u32>,
    pub entry_discard: bool,
    pub layer_entry: bool,
    /// vs_layer_entry's `{'loc', 'comp'}` (the reflection's `layer_from`)
    pub layer_from: Option<(u32, char)>,
    /// vertex shader: `vs_viewport_mask` lowered a draw-uniform viewport mask (the reflection's `viewport_mask`)
    pub viewport_mask: bool,
    /// pixel shader: the typed-output entry point holds the alpha test / the logic operation (`dmsh_entry.typed_ops`: the reflection's `entry_u_alpha` / `entry_u_logic`)
    pub typed_alpha: bool,
    pub typed_logic: bool,
    /// compute: first_store's `[(storage binding's MSL name, word, value)]` (the reflection's `first_store`, with the binding's `reg` looked up there)
    pub first_store: Vec<(String, i64, i64)>,
    /// compute: tg_spill's bytes per workgroup (0: unchanged)
    pub tg_spill: u64,
    /// compute: compute_loops' contained loops (the reflection's `loop_sites`)
    pub loop_sites: Vec<cs::LoopSite>,
}

impl PostError {
    /// the `status` string `dmsh_build.finish()` returns when it catches this failure (what the build log and the summary print); "exception" for a Python exception `finish()` does not
    /// catch (SystemExit / ValueError out of a step that has no handler: the build itself stops there)
    pub fn status(&self) -> &'static str {
        match self {
            PostError::Unported(_) => "unported",
            PostError::Failed { step, .. } => match *step {
                "check_msl" => "image atomic MSL failed",
                "vs_viewport_mask" => "vertex viewport mask failed",
                "per_vertex" => "per-vertex input rewrite failed",
                "select_guards" => "uniform bounds rewrite failed",
                "gather_offsets" => "gather offsets rewrite failed",
                "hmma_msl" => "HMMA rewrite failed",
                "shared_store_msl" => "shared store rewrite failed",
                "pool_arrays" => "pool rewrite failed",
                "uniform_tests" => "uniform tests rewrite failed",
                "tg_spill" => "shared memory rewrite failed",
                "compute_loops" => "compute loop containment failed",
                _ => "exception",
            },
        }
    }
}

macro_rules! step {
    ($cur:ident, $e:expr) => {{
        let n: Option<String> = match $e {
            Cow::Owned(o) => Some(o),
            Cow::Borrowed(_) => None,
        };
        if let Some(o) = n {
            $cur = Cow::Owned(o);
        }
    }};
}

/// `dmsh_chain.storage_constant_mode()`: DMSH_SCONST, default `fp`; another value gives another chain id, which the host does not load
pub const STORAGE_CONSTANT_MODE: &str = "fp";

pub fn post_rewrites(src: &str, job: &Job) -> Result<PostOut, PostError> {
    if let Stage::Other(_) = job.stage {
        return Err(PostError::Unported("stage"));
    }
    let ps = job.stage == Stage::Ps;
    let mut cur: Cow<str> = Cow::Borrowed(src);
    step!(cur, steps::check_msl(&cur)?);
    if ps {
        step!(cur, pervertex::per_vertex(&cur)?);
    }
    step!(cur, steps::precise(&cur));
    step!(cur, bit0::bit0_mask(&cur));
    step!(cur, guards::select_guards(&cur)?);
    step!(cur, ampere::mul_extended(&cur));
    step!(cur, ampere::float_add_msl(&cur));
    step!(cur, ampere::gather_offsets(&cur)?);
    step!(cur, hmma::hmma_msl(&cur)?);
    step!(cur, ampere::shared_store_msl(&cur)?);
    step!(cur, pool::pool_arrays(&cur)?);
    step!(cur, wide::wide_loads(&cur));
    if let Some(res) = job.res {
        // the storage buffers the translator found read-only, by their MSL number (binding order), the raw pool table left out
        let mut sb: Vec<&serde_json::Value> = res
            .get("sbufs")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        sb.sort_by_key(|b| b.get("binding").and_then(|x| x.as_i64()).unwrap_or(0));
        let read_only: std::collections::HashSet<u32> = sb
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                b.get("flags").and_then(|x| x.as_i64()).unwrap_or(0) & 1 == 0
                    && b.get("cb_slot").and_then(|x| x.as_i64())
                        != Some(crate::finish::abi::GMEM_MAP_CB_SLOT)
            })
            .map(|(i, _)| i as u32)
            .collect();
        step!(
            cur,
            wide::constant_reads(&cur, &read_only, STORAGE_CONSTANT_MODE)
        );
    }
    step!(cur, steps::tg_fence(&cur)?);
    step!(cur, border::border_emu(&cur)?);
    if ps {
        let (n, _) = zsnap::frag_z_snap(&cur);
        step!(cur, n);
    }
    step!(cur, utest::uniform_tests(&cur)?);
    let mut blend0 = 0;
    if ps {
        let (n, k) = blend::blend_zero(&cur)?;
        blend0 = k;
        step!(cur, n);
    }
    let mut res = PostOut {
        blend_zero: blend0,
        ..Default::default()
    };
    match job.stage {
        Stage::Cs if job.mesh => res.out = cur.into_owned(),
        Stage::Hs | Stage::Ds => res.out = cur.into_owned(),
        Stage::Cs => {
            let local = job.local.as_deref();
            let (s, first) = cs::first_store(&cur, local);
            res.first_store = first;
            let (s, spill) = cs::tg_spill(&s)?;
            res.tg_spill = spill;
            let (s, sites) = cs::compute_loops(&s)?;
            res.loop_sites = sites;
            let mut text = s;
            if let Cow::Owned(o) = steps::max_threads(&text, local) {
                text = o;
            }
            if let Cow::Owned(o) = steps::raw_noinline(&text, "cs") {
                text = o;
            }
            res.out = text;
        }
        Stage::Vs => {
            step!(cur, steps::raw_noinline(&cur, "vs"));
            let mut s = vs::vs_init(&cur);
            s = vs::vs_layer(s)?;
            s = vs::vs_base_instance(&s);
            if let Some(partner) = job.partner_locs {
                let (n, pad) = vs::vs_pad(&s, partner);
                s = n;
                res.vs_pad = pad;
            }
            let Some(answer) = job.res else {
                return Err(PostError::Unported(
                    "vs_writes without the translator's answer json",
                ));
            };
            let e = vs::vertex_entries(&s, answer, job.has_tf, job.tf)?;
            res.entry_discard = e.discard;
            res.viewport_mask = e.viewport_mask;
            res.plain_len = Some(e.plain_len);
            res.layer_entry = e.layer_from.is_some();
            res.layer_from = e.layer_from;
            res.out = e.full;
            res.analysis = Some(e.single);
        }
        Stage::Ps => {
            step!(cur, steps::raw_noinline(&cur, "ps"));
            let fu = fs_uint(&cur);
            let fd = if job.dual_want { fs_dual(&cur) } else { None };
            let shared = if fu.is_some() || fd.is_some() {
                side::shared_body(&cur)
            } else {
                None
            };
            res.fs_uint = fu.is_some() && shared.is_some();
            if res.fs_uint
                && let Some(u) = fu.as_deref()
            {
                (res.typed_alpha, res.typed_logic) = crate::fsuint::typed_ops(u);
            }
            res.fs_dual = fd.is_some() && shared.is_some();
            match shared {
                Some(mut sh) => {
                    res.shared_body = true;
                    sh.push_str(fu.as_deref().unwrap_or(""));
                    sh.push_str(fd.as_deref().unwrap_or(""));
                    res.analysis = Some(cur.into_owned());
                    res.out = sh;
                }
                None => res.out = cur.into_owned(),
            }
        }
        Stage::Other(_) => unreachable!(),
    }
    Ok(res)
}
